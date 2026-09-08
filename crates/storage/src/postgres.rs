//! PostgreSQL [`MetadataStore`].
//!
//! Standard PostgreSQL only. It runs against a `postgres:17` container in CI and
//! against a Neon branch in production, and no business logic depends on which.
//!
//! Queries are written with the runtime `sqlx::query` API rather than the
//! compile-time macros on purpose: the macros need a live database (or a checked
//! `.sqlx` cache) at *build* time, which would make `cargo build` fail on a
//! laptop with no database and turn every CI job into a database job.

use async_trait::async_trait;
use rustly_common::{Error, Result, Timestamp};
use rustly_domain::submission::JobId;
use rustly_domain::trial::TrialId;
use rustly_domain::user::UserId;
use rustly_domain::{
    Clan, ClanTag, Difficulty, EventKind, Level, PlatformEvent, RankScore, Submission,
    SubmissionId, SubmissionState, Trial, TrialLifecycle, TrialSlug, TrialStatus, TrialTopic, User,
    Username, Verdict,
};
use rustly_progress::{Checkpoint, Completion, MergeOutcome, MergedEntry, ProgressState};
use rustly_protocol::broker::{ExecutionLimits, TrustClass};
use rustly_ranking::Solve;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::finalize::{self, Context};
use crate::store::{
    LeasedJob, MetadataStore, NewSubmission, SubmissionOutcome, TrialFilter, TrialUserState,
};

fn db(source: sqlx::Error) -> Error {
    Error::dependency("postgres", source)
}

fn ts(value: OffsetDateTime) -> Timestamp {
    Timestamp::from_offset(value)
}

/// Serialise a serde enum to its snake_case wire form for storage.
fn tag<T: serde::Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value).map_err(|e| Error::Internal(e.to_string()))? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(Error::Internal(format!(
            "expected a string tag, got {other}"
        ))),
    }
}

/// Parse a stored snake_case tag back into its enum.
fn untag<T: serde::de::DeserializeOwned>(field: &'static str, raw: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(raw.to_owned()))
        .map_err(|_| Error::Internal(format!("unrecognised {field} in database: {raw:?}")))
}

/// A PostgreSQL-backed metadata store.
#[derive(Debug, Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    /// Connect with a bounded pool.
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(url)
            .await
            .map_err(db)?;
        Ok(Self { pool })
    }

    /// Wrap an existing pool.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Apply the embedded migrations.
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("../../migrations")
            .run(&self.pool)
            .await
            .map_err(|e| Error::dependency("postgres-migrate", e))
    }

    /// The underlying pool, for diagnostics.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    async fn load_user(&self, id: UserId) -> Result<User> {
        let row = sqlx::query(
            "SELECT u.id, u.username, u.rank_score, u.level, u.trials_solved, u.supporter,
                    u.created_at, c.tag AS clan_tag
             FROM users u
             LEFT JOIN clans c ON c.id = u.clan_id
             WHERE u.id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?
        .ok_or_else(|| Error::not_found("user", id))?;
        user_from_row(&row)
    }
}

fn user_from_row(row: &sqlx::postgres::PgRow) -> Result<User> {
    let clan_tag: Option<String> = row.try_get("clan_tag").map_err(db)?;
    Ok(User {
        id: UserId::from_uuid(row.try_get::<Uuid, _>("id").map_err(db)?),
        username: Username::parse(row.try_get::<String, _>("username").map_err(db)?.as_str())?,
        clan: clan_tag.as_deref().map(ClanTag::parse).transpose()?,
        rank_score: RankScore(row.try_get::<f64, _>("rank_score").map_err(db)?),
        level: Level(row.try_get::<i32, _>("level").map_err(db)? as u32),
        trials_solved: row.try_get::<i32, _>("trials_solved").map_err(db)? as u32,
        supporter: row.try_get("supporter").map_err(db)?,
        created_at: ts(row.try_get("created_at").map_err(db)?),
    })
}

fn trial_from_row(row: &sqlx::postgres::PgRow) -> Result<Trial> {
    let topics: Vec<String> = row.try_get("topics").map_err(db)?;
    Ok(Trial {
        id: TrialId::from_uuid(row.try_get::<Uuid, _>("id").map_err(db)?),
        slug: TrialSlug::parse(row.try_get::<String, _>("slug").map_err(db)?.as_str())?,
        title: row.try_get("title").map_err(db)?,
        difficulty: untag::<Difficulty>(
            "difficulty",
            row.try_get::<String, _>("difficulty").map_err(db)?.as_str(),
        )?,
        topics: topics
            .iter()
            .map(|t| untag::<TrialTopic>("topic", t))
            .collect::<Result<Vec<_>>>()?,
        lifecycle: untag::<TrialLifecycle>(
            "lifecycle",
            row.try_get::<String, _>("lifecycle").map_err(db)?.as_str(),
        )?,
        version: row.try_get::<i32, _>("version").map_err(db)? as u32,
        content_cid: row.try_get("content_cid").map_err(db)?,
        published_at: ts(row.try_get("published_at").map_err(db)?),
    })
}

fn submission_from_row(row: &sqlx::postgres::PgRow) -> Result<Submission> {
    let state: serde_json::Value = row.try_get("state").map_err(db)?;
    Ok(Submission {
        id: SubmissionId::from_uuid(row.try_get::<Uuid, _>("id").map_err(db)?),
        job_id: JobId::from_uuid(row.try_get::<Uuid, _>("job_id").map_err(db)?),
        user_id: UserId::from_uuid(row.try_get::<Uuid, _>("user_id").map_err(db)?),
        trial_id: TrialId::from_uuid(row.try_get::<Uuid, _>("trial_id").map_err(db)?),
        trial_version: row.try_get::<i32, _>("trial_version").map_err(db)? as u32,
        source_cid: row.try_get("source_cid").map_err(db)?,
        state: serde_json::from_value(state)
            .map_err(|e| Error::Internal(format!("corrupt submission state: {e}")))?,
        created_at: ts(row.try_get("created_at").map_err(db)?),
        finished_at: row
            .try_get::<Option<OffsetDateTime>, _>("finished_at")
            .map_err(db)?
            .map(ts),
    })
}

#[async_trait]
impl MetadataStore for PostgresStore {
    async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn create_user(&self, username: &Username) -> Result<User> {
        let id = Uuid::now_v7();
        let result = sqlx::query(
            "INSERT INTO users (id, username, username_key) VALUES ($1, $2, $3)
             ON CONFLICT (username_key) DO NOTHING",
        )
        .bind(id)
        .bind(username.as_str())
        .bind(username.lookup_key())
        .execute(&self.pool)
        .await
        .map_err(db)?;

        if result.rows_affected() == 0 {
            return Err(Error::Conflict(format!("username `{username}` is taken")));
        }
        self.load_user(UserId::from_uuid(id)).await
    }

    async fn user_by_username(&self, username: &str) -> Result<User> {
        let row = sqlx::query(
            "SELECT u.id, u.username, u.rank_score, u.level, u.trials_solved, u.supporter,
                    u.created_at, c.tag AS clan_tag
             FROM users u
             LEFT JOIN clans c ON c.id = u.clan_id
             WHERE u.username_key = $1",
        )
        .bind(username.to_ascii_lowercase())
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?
        .ok_or_else(|| Error::not_found("user", username))?;
        user_from_row(&row)
    }

    async fn user_by_id(&self, id: UserId) -> Result<User> {
        self.load_user(id).await
    }

    async fn global_rank(&self, id: UserId) -> Result<Option<u64>> {
        let row = sqlx::query("SELECT rank_score FROM users WHERE id = $1")
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?
            .ok_or_else(|| Error::not_found("user", id))?;
        let score: f64 = row.try_get("rank_score").map_err(db)?;
        if score <= 0.0 {
            return Ok(None);
        }
        let ahead: i64 = sqlx::query("SELECT count(*) AS n FROM users WHERE rank_score > $1")
            .bind(score)
            .fetch_one(&self.pool)
            .await
            .map_err(db)?
            .try_get("n")
            .map_err(db)?;
        Ok(Some(ahead as u64 + 1))
    }

    async fn put_trial(&self, trial: &Trial) -> Result<()> {
        let topics: Vec<String> = trial.topics.iter().map(tag).collect::<Result<Vec<_>>>()?;
        sqlx::query(
            "INSERT INTO trials (id, slug, title, difficulty, topics, lifecycle, version,
                                 content_cid, published_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (slug) DO UPDATE SET
                 title = EXCLUDED.title,
                 difficulty = EXCLUDED.difficulty,
                 topics = EXCLUDED.topics,
                 lifecycle = EXCLUDED.lifecycle,
                 version = EXCLUDED.version,
                 content_cid = EXCLUDED.content_cid",
        )
        .bind(trial.id.as_uuid())
        .bind(trial.slug.as_str())
        .bind(&trial.title)
        .bind(tag(&trial.difficulty)?)
        .bind(&topics)
        .bind(tag(&trial.lifecycle)?)
        .bind(trial.version as i32)
        .bind(&trial.content_cid)
        .bind(trial.published_at.as_offset())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn trial_by_slug(&self, slug: &TrialSlug) -> Result<Trial> {
        let row = sqlx::query("SELECT * FROM trials WHERE slug = $1")
            .bind(slug.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?
            .ok_or_else(|| Error::not_found("trial", slug))?;
        trial_from_row(&row)
    }

    async fn list_trials(&self, filter: &TrialFilter) -> Result<Vec<Trial>> {
        let difficulty = filter.difficulty.as_ref().map(tag).transpose()?;
        let topic = filter.topic.as_ref().map(tag).transpose()?;
        let limit = if filter.limit == 0 {
            200
        } else {
            filter.limit.min(500)
        } as i64;

        let rows = sqlx::query(
            "SELECT * FROM trials
             WHERE lifecycle <> 'draft'
               AND ($1::text IS NULL OR difficulty = $1)
               AND ($2::text IS NULL OR $2 = ANY(topics))
             ORDER BY slug
             LIMIT $3",
        )
        .bind(difficulty)
        .bind(topic)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        rows.iter().map(trial_from_row).collect()
    }

    async fn trial_user_state(&self, user_id: UserId, trial_id: TrialId) -> Result<TrialUserState> {
        let row = sqlx::query(
            "SELECT status, attempts, revealed_solutions FROM trial_user_state
             WHERE user_id = $1 AND trial_id = $2",
        )
        .bind(user_id.as_uuid())
        .bind(trial_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;

        let Some(row) = row else {
            return Ok(TrialUserState::default());
        };
        Ok(TrialUserState {
            status: untag::<TrialStatus>(
                "status",
                row.try_get::<String, _>("status").map_err(db)?.as_str(),
            )?,
            attempts: row.try_get::<i32, _>("attempts").map_err(db)? as u32,
            revealed_solutions: row.try_get("revealed_solutions").map_err(db)?,
        })
    }

    async fn reveal_solutions(&self, user_id: UserId, trial_id: TrialId) -> Result<()> {
        sqlx::query(
            "INSERT INTO trial_user_state (user_id, trial_id, revealed_solutions)
             VALUES ($1, $2, TRUE)
             ON CONFLICT (user_id, trial_id) DO UPDATE SET revealed_solutions = TRUE",
        )
        .bind(user_id.as_uuid())
        .bind(trial_id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn merge_checkpoint(
        &self,
        user_id: UserId,
        checkpoint: &Checkpoint,
    ) -> Result<MergeOutcome> {
        checkpoint.validate()?;
        let mut current = self.progress(user_id).await?;
        let before = current.clone();
        let outcome = current.merge(checkpoint);

        let mut tx = self.pool.begin().await.map_err(db)?;
        for (key, entry) in &current.entries {
            if before.entries.get(key) == Some(entry) {
                continue;
            }
            sqlx::query(
                "INSERT INTO progress_entries (user_id, key, revision, completion, recorded_at)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (user_id, key) DO UPDATE SET
                     revision = GREATEST(progress_entries.revision, EXCLUDED.revision),
                     completion = EXCLUDED.completion,
                     recorded_at = GREATEST(progress_entries.recorded_at, EXCLUDED.recorded_at)",
            )
            .bind(user_id.as_uuid())
            .bind(key)
            .bind(entry.revision as i64)
            .bind(tag(&entry.completion)?)
            .bind(entry.recorded_at.as_offset())
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        Ok(outcome)
    }

    async fn progress(&self, user_id: UserId) -> Result<ProgressState> {
        let rows = sqlx::query(
            "SELECT key, revision, completion, recorded_at FROM progress_entries WHERE user_id = $1",
        )
        .bind(user_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        let mut state = ProgressState::default();
        for row in &rows {
            state.entries.insert(
                row.try_get::<String, _>("key").map_err(db)?,
                MergedEntry {
                    revision: row.try_get::<i64, _>("revision").map_err(db)? as u64,
                    completion: untag::<Completion>(
                        "completion",
                        row.try_get::<String, _>("completion").map_err(db)?.as_str(),
                    )?,
                    recorded_at: ts(row.try_get("recorded_at").map_err(db)?),
                },
            );
        }
        Ok(state)
    }

    async fn create_submission(&self, new: NewSubmission) -> Result<(Submission, bool)> {
        let id = Uuid::now_v7();
        let job_id = Uuid::now_v7();
        let state = serde_json::to_value(SubmissionState::Queued)
            .map_err(|e| Error::Internal(e.to_string()))?;

        let mut tx = self.pool.begin().await.map_err(db)?;
        let inserted = sqlx::query(
            "INSERT INTO submissions
                 (id, job_id, user_id, trial_id, trial_version, source_cid, state, idempotency_key)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (user_id, idempotency_key) DO NOTHING
             RETURNING *",
        )
        .bind(id)
        .bind(job_id)
        .bind(new.user_id.as_uuid())
        .bind(new.trial_id.as_uuid())
        .bind(new.trial_version as i32)
        .bind(&new.source_cid)
        .bind(&state)
        .bind(&new.idempotency_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;

        if let Some(row) = inserted {
            sqlx::query("INSERT INTO job_queue (job_id) VALUES ($1)")
                .bind(job_id)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            tx.commit().await.map_err(db)?;
            return Ok((submission_from_row(&row)?, false));
        }
        tx.rollback().await.map_err(db)?;

        let row =
            sqlx::query("SELECT * FROM submissions WHERE user_id = $1 AND idempotency_key = $2")
                .bind(new.user_id.as_uuid())
                .bind(&new.idempotency_key)
                .fetch_one(&self.pool)
                .await
                .map_err(db)?;
        Ok((submission_from_row(&row)?, true))
    }

    async fn submission(&self, id: SubmissionId) -> Result<Submission> {
        let row = sqlx::query("SELECT * FROM submissions WHERE id = $1")
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?
            .ok_or_else(|| Error::not_found("submission", id))?;
        submission_from_row(&row)
    }

    async fn update_submission_state(&self, job_id: JobId, state: SubmissionState) -> Result<()> {
        let encoded = serde_json::to_value(&state).map_err(|e| Error::Internal(e.to_string()))?;
        // `verdict IS NULL` is the terminal guard: a late heartbeat cannot
        // un-finish a finished submission.
        let result =
            sqlx::query("UPDATE submissions SET state = $2 WHERE job_id = $1 AND verdict IS NULL")
                .bind(job_id.as_uuid())
                .bind(encoded)
                .execute(&self.pool)
                .await
                .map_err(db)?;

        if result.rows_affected() == 0 {
            let exists: i64 =
                sqlx::query("SELECT count(*) AS n FROM submissions WHERE job_id = $1")
                    .bind(job_id.as_uuid())
                    .fetch_one(&self.pool)
                    .await
                    .map_err(db)?
                    .try_get("n")
                    .map_err(db)?;
            if exists == 0 {
                return Err(Error::not_found("job", job_id));
            }
        }
        Ok(())
    }

    async fn lease_jobs(
        &self,
        worker_id: &str,
        trust_class: TrustClass,
        capacity: u32,
        lease_seconds: i64,
    ) -> Result<Vec<LeasedJob>> {
        let expires = Timestamp::now().plus_seconds(lease_seconds);
        let mut tx = self.pool.begin().await.map_err(db)?;

        // SKIP LOCKED gives at-most-once dispatch under concurrent leasing
        // without a distributed lock. Expired leases are reclaimed here too.
        let rows = sqlx::query(
            "WITH claimed AS (
                 SELECT job_id FROM job_queue
                 WHERE worker_id IS NULL OR lease_expires_at < now()
                 ORDER BY enqueued_at
                 FOR UPDATE SKIP LOCKED
                 LIMIT $1
             )
             UPDATE job_queue q
                SET worker_id = $2, worker_trust = $3, lease_expires_at = $4
               FROM claimed
              WHERE q.job_id = claimed.job_id
              RETURNING q.job_id",
        )
        .bind(capacity.min(64) as i64)
        .bind(worker_id)
        .bind(tag(&trust_class)?)
        .bind(expires.as_offset())
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;

        let mut leased = Vec::with_capacity(rows.len());
        for row in &rows {
            let job_id: Uuid = row.try_get("job_id").map_err(db)?;
            let detail = sqlx::query(
                "SELECT s.source_cid, s.trial_version, t.content_cid
                 FROM submissions s JOIN trials t ON t.id = s.trial_id
                 WHERE s.job_id = $1",
            )
            .bind(job_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;

            let dispatched = serde_json::to_value(SubmissionState::Dispatched {
                worker_id: worker_id.to_owned(),
            })
            .map_err(|e| Error::Internal(e.to_string()))?;
            sqlx::query("UPDATE submissions SET state = $2 WHERE job_id = $1 AND verdict IS NULL")
                .bind(job_id)
                .bind(dispatched)
                .execute(&mut *tx)
                .await
                .map_err(db)?;

            leased.push(LeasedJob {
                job_id: JobId::from_uuid(job_id),
                source_cid: detail.try_get("source_cid").map_err(db)?,
                trial_package_cid: detail.try_get("content_cid").map_err(db)?,
                trial_version: detail.try_get::<i32, _>("trial_version").map_err(db)? as u32,
                limits: ExecutionLimits::default(),
                // Invariant: hidden tests only ever reach a Trusted worker.
                may_receive_hidden_tests: trust_class.may_receive_hidden_tests(),
                lease_expires_at: expires,
            });
        }

        tx.commit().await.map_err(db)?;
        Ok(leased)
    }

    async fn record_result(
        &self,
        job_id: JobId,
        worker_id: &str,
        verdict: Verdict,
        result_manifest_hash: &str,
    ) -> Result<SubmissionOutcome> {
        let mut tx = self.pool.begin().await.map_err(db)?;

        // Lock the submission row for the whole decision so two workers
        // reporting simultaneously cannot both finalise.
        let submission_row = sqlx::query("SELECT * FROM submissions WHERE job_id = $1 FOR UPDATE")
            .bind(job_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
            .ok_or_else(|| Error::not_found("job", job_id))?;
        let submission = submission_from_row(&submission_row)?;

        let holder: Option<String> =
            sqlx::query("SELECT worker_id FROM job_queue WHERE job_id = $1")
                .bind(job_id.as_uuid())
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?
                .map(|r| r.try_get("worker_id"))
                .transpose()
                .map_err(db)?
                .flatten();
        if let Some(holder) = holder {
            if holder != worker_id {
                return Err(Error::Forbidden("job is leased by another worker".into()));
            }
        }

        let trial_row = sqlx::query("SELECT * FROM trials WHERE id = $1")
            .bind(submission.trial_id.as_uuid())
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
        let trial = trial_from_row(&trial_row)?;

        let state_row = sqlx::query(
            "SELECT status, attempts, revealed_solutions FROM trial_user_state
             WHERE user_id = $1 AND trial_id = $2",
        )
        .bind(submission.user_id.as_uuid())
        .bind(trial.id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        let user_state = match &state_row {
            None => TrialUserState::default(),
            Some(row) => TrialUserState {
                status: untag::<TrialStatus>(
                    "status",
                    row.try_get::<String, _>("status").map_err(db)?.as_str(),
                )?,
                attempts: row.try_get::<i32, _>("attempts").map_err(db)? as u32,
                revealed_solutions: row.try_get("revealed_solutions").map_err(db)?,
            },
        };

        let user_row = sqlx::query(
            "SELECT u.id, u.username, u.rank_score, u.level, u.trials_solved, u.supporter,
                    u.experience, u.created_at, c.tag AS clan_tag
             FROM users u LEFT JOIN clans c ON c.id = u.clan_id
             WHERE u.id = $1 FOR UPDATE OF u",
        )
        .bind(submission.user_id.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        let user = user_from_row(&user_row)?;
        let experience: i64 = user_row.try_get("experience").map_err(db)?;

        let solve_rows = sqlx::query(
            "SELECT difficulty, lifecycle, revealed_solutions FROM solves WHERE user_id = $1",
        )
        .bind(submission.user_id.as_uuid())
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
        let mut existing_solves = Vec::with_capacity(solve_rows.len());
        for row in &solve_rows {
            existing_solves.push(Solve {
                difficulty: untag::<Difficulty>(
                    "difficulty",
                    row.try_get::<String, _>("difficulty").map_err(db)?.as_str(),
                )?,
                lifecycle: untag::<TrialLifecycle>(
                    "lifecycle",
                    row.try_get::<String, _>("lifecycle").map_err(db)?.as_str(),
                )?,
                revealed_solutions: row.try_get("revealed_solutions").map_err(db)?,
            });
        }

        let context = Context {
            verdict,
            already_finished: submission.state.is_terminal(),
            existing_verdict: submission.state.verdict(),
            current_status: user_state.status,
            lifecycle: trial.lifecycle,
            difficulty: trial.difficulty,
            revealed_solutions: user_state.revealed_solutions,
            existing_solves,
            existing_experience: experience as u64,
        };
        let plan = finalize::plan(&context, user.trials_solved);

        if !plan.accepted {
            tx.rollback().await.map_err(db)?;
            return Ok(SubmissionOutcome {
                accepted: false,
                recorded_verdict: plan.recorded_verdict,
                first_solve: false,
                user,
            });
        }

        let finished = serde_json::to_value(SubmissionState::Finished {
            verdict: plan.recorded_verdict,
        })
        .map_err(|e| Error::Internal(e.to_string()))?;
        sqlx::query(
            "UPDATE submissions
                SET state = $2, verdict = $3, result_manifest_hash = $4, finished_at = now()
              WHERE job_id = $1",
        )
        .bind(job_id.as_uuid())
        .bind(finished)
        .bind(plan.recorded_verdict.code())
        .bind(result_manifest_hash)
        .execute(&mut *tx)
        .await
        .map_err(db)?;

        sqlx::query("DELETE FROM job_queue WHERE job_id = $1")
            .bind(job_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(db)?;

        sqlx::query(
            "INSERT INTO trial_user_state (user_id, trial_id, status, attempts)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (user_id, trial_id) DO UPDATE SET
                 status = EXCLUDED.status,
                 attempts = trial_user_state.attempts + $4",
        )
        .bind(submission.user_id.as_uuid())
        .bind(trial.id.as_uuid())
        .bind(tag(&plan.new_status)?)
        .bind(i32::from(plan.increment_attempts))
        .execute(&mut *tx)
        .await
        .map_err(db)?;

        if plan.first_solve {
            sqlx::query(
                "INSERT INTO solves (user_id, trial_id, difficulty, lifecycle, revealed_solutions)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (user_id, trial_id) DO NOTHING",
            )
            .bind(submission.user_id.as_uuid())
            .bind(trial.id.as_uuid())
            .bind(tag(&trial.difficulty)?)
            .bind(tag(&trial.lifecycle)?)
            .bind(user_state.revealed_solutions)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }

        if plan.increment_trials_solved || plan.new_rank_score.is_some() {
            sqlx::query(
                "UPDATE users SET
                     trials_solved = trials_solved + $2,
                     rank_score = COALESCE($3, rank_score),
                     experience = COALESCE($4, experience),
                     level = COALESCE($5, level)
                 WHERE id = $1",
            )
            .bind(submission.user_id.as_uuid())
            .bind(i32::from(plan.increment_trials_solved))
            .bind(plan.new_rank_score.map(|s| s.0))
            .bind(plan.new_experience.map(|x| x as i64))
            .bind(plan.new_level.map(|l| l.0 as i32))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }

        tx.commit().await.map_err(db)?;

        Ok(SubmissionOutcome {
            accepted: true,
            recorded_verdict: plan.recorded_verdict,
            first_solve: plan.first_solve,
            user: self.load_user(submission.user_id).await?,
        })
    }

    async fn solves(&self, user_id: UserId) -> Result<Vec<Solve>> {
        let rows = sqlx::query(
            "SELECT difficulty, lifecycle, revealed_solutions FROM solves WHERE user_id = $1",
        )
        .bind(user_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        rows.iter()
            .map(|row| {
                Ok(Solve {
                    difficulty: untag::<Difficulty>(
                        "difficulty",
                        row.try_get::<String, _>("difficulty").map_err(db)?.as_str(),
                    )?,
                    lifecycle: untag::<TrialLifecycle>(
                        "lifecycle",
                        row.try_get::<String, _>("lifecycle").map_err(db)?.as_str(),
                    )?,
                    revealed_solutions: row.try_get("revealed_solutions").map_err(db)?,
                })
            })
            .collect()
    }

    async fn append_event(&self, event: &PlatformEvent) -> Result<()> {
        sqlx::query(
            "INSERT INTO events (id, kind, summary, href, subject, occurred_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(event.id.as_uuid())
        .bind(tag(&event.kind)?)
        .bind(&event.summary)
        .bind(&event.href)
        .bind(&event.subject)
        .bind(event.occurred_at.as_offset())
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn events(
        &self,
        since: Option<Timestamp>,
        kind: Option<EventKind>,
        limit: usize,
    ) -> Result<Vec<PlatformEvent>> {
        let rows = sqlx::query(
            "SELECT * FROM events
             WHERE ($1::timestamptz IS NULL OR occurred_at >= $1)
               AND ($2::text IS NULL OR kind = $2)
             ORDER BY occurred_at DESC
             LIMIT $3",
        )
        .bind(since.map(|t| t.as_offset()))
        .bind(kind.as_ref().map(tag).transpose()?)
        .bind(limit.clamp(1, 500) as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        rows.iter()
            .map(|row| {
                Ok(PlatformEvent {
                    id: rustly_domain::event::EventId::from_uuid(
                        row.try_get::<Uuid, _>("id").map_err(db)?,
                    ),
                    kind: untag::<EventKind>(
                        "kind",
                        row.try_get::<String, _>("kind").map_err(db)?.as_str(),
                    )?,
                    summary: row.try_get("summary").map_err(db)?,
                    href: row.try_get("href").map_err(db)?,
                    subject: row.try_get("subject").map_err(db)?,
                    occurred_at: ts(row.try_get("occurred_at").map_err(db)?),
                })
            })
            .collect()
    }

    async fn create_clan(&self, clan: &Clan) -> Result<()> {
        let result = sqlx::query(
            "INSERT INTO clans (id, tag, name, blurb, owner_id, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (tag) DO NOTHING",
        )
        .bind(clan.id.as_uuid())
        .bind(clan.tag.as_str())
        .bind(&clan.name)
        .bind(&clan.blurb)
        .bind(clan.owner.as_uuid())
        .bind(clan.created_at.as_offset())
        .execute(&self.pool)
        .await
        .map_err(db)?;

        if result.rows_affected() == 0 {
            return Err(Error::Conflict(format!("clan tag `{}` is taken", clan.tag)));
        }
        Ok(())
    }

    async fn add_clan_member(&self, tag: &ClanTag, user_id: UserId) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let clan_id: Option<Uuid> = sqlx::query("SELECT id FROM clans WHERE tag = $1")
            .bind(tag.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
            .map(|r| r.try_get("id"))
            .transpose()
            .map_err(db)?;
        let clan_id = clan_id.ok_or_else(|| Error::not_found("clan", tag))?;

        sqlx::query(
            "INSERT INTO clan_members (clan_id, user_id) VALUES ($1, $2)
             ON CONFLICT (clan_id, user_id) DO NOTHING",
        )
        .bind(clan_id)
        .bind(user_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(db)?;

        sqlx::query("UPDATE users SET clan_id = $2 WHERE id = $1")
            .bind(user_id.as_uuid())
            .bind(clan_id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;

        tx.commit().await.map_err(db)?;
        Ok(())
    }

    async fn clan_by_tag(&self, tag: &ClanTag) -> Result<Clan> {
        let row = sqlx::query("SELECT * FROM clans WHERE tag = $1")
            .bind(tag.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?
            .ok_or_else(|| Error::not_found("clan", tag))?;

        Ok(Clan {
            id: rustly_domain::clan::ClanId::from_uuid(row.try_get::<Uuid, _>("id").map_err(db)?),
            tag: ClanTag::parse(row.try_get::<String, _>("tag").map_err(db)?.as_str())?,
            name: row.try_get("name").map_err(db)?,
            blurb: row.try_get("blurb").map_err(db)?,
            owner: UserId::from_uuid(row.try_get::<Uuid, _>("owner_id").map_err(db)?),
            created_at: ts(row.try_get("created_at").map_err(db)?),
        })
    }

    async fn clan_members(&self, tag: &ClanTag) -> Result<Vec<Username>> {
        let exists: i64 = sqlx::query("SELECT count(*) AS n FROM clans WHERE tag = $1")
            .bind(tag.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(db)?
            .try_get("n")
            .map_err(db)?;
        if exists == 0 {
            return Err(Error::not_found("clan", tag));
        }

        let rows = sqlx::query(
            "SELECT u.username FROM clan_members m
             JOIN users u ON u.id = m.user_id
             JOIN clans c ON c.id = m.clan_id
             WHERE c.tag = $1
             ORDER BY u.username_key",
        )
        .bind(tag.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        rows.iter()
            .map(|row| Username::parse(row.try_get::<String, _>("username").map_err(db)?.as_str()))
            .collect()
    }
}
