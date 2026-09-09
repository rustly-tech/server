//! In-memory [`MetadataStore`].
//!
//! Not a toy: it implements the same rules as the PostgreSQL backend by calling
//! the same [`crate::finalize::plan`] function, and it is what the API
//! integration tests run against. That is deliberate - it makes the whole HTTP
//! surface testable without a database, which keeps `cargo test` hermetic.
//!
//! It holds everything behind a single `RwLock`. That is the right trade for a
//! test and local-development backend; production uses PostgreSQL.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use rustly_common::{Error, Result, Timestamp};
use rustly_domain::submission::JobId;
use rustly_domain::trial::TrialId;
use rustly_domain::user::UserId;
use rustly_domain::{
    Clan, ClanTag, EventKind, Level, PlatformEvent, RankScore, Submission, SubmissionId,
    SubmissionState, Trial, TrialSlug, User, Username, Verdict,
};
use rustly_progress::{Checkpoint, MergeOutcome, ProgressState};
use rustly_protocol::broker::{ExecutionLimits, TrustClass};
use rustly_ranking::Solve;
use tokio::sync::RwLock;

use crate::finalize::{self, Context};
use crate::store::{
    LeasedJob, MetadataStore, NewSubmission, SubmissionOutcome, TrialFilter, TrialUserState,
};

#[derive(Debug, Default)]
struct Tables {
    users: HashMap<UserId, User>,
    username_index: HashMap<String, UserId>,
    experience: HashMap<UserId, u64>,
    trials: HashMap<TrialId, Trial>,
    slug_index: HashMap<String, TrialId>,
    trial_user_state: HashMap<(UserId, TrialId), TrialUserState>,
    solves: HashMap<UserId, Vec<Solve>>,
    progress: HashMap<UserId, ProgressState>,
    submissions: HashMap<SubmissionId, Submission>,
    job_index: HashMap<JobId, SubmissionId>,
    idempotency: HashMap<(UserId, String), SubmissionId>,
    queue: Vec<JobId>,
    leases: HashMap<JobId, String>,
    events: Vec<PlatformEvent>,
    clans: HashMap<String, Clan>,
    clan_members: HashMap<String, Vec<UserId>>,
}

/// An in-memory metadata store.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    tables: Arc<RwLock<Tables>>,
}

impl MemoryStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of jobs waiting to be leased. Exposed for queue-depth metrics.
    pub async fn queue_depth(&self) -> usize {
        self.tables.read().await.queue.len()
    }
}

#[async_trait]
impl MetadataStore for MemoryStore {
    async fn ping(&self) -> Result<()> {
        Ok(())
    }

    async fn create_user(&self, username: &Username) -> Result<User> {
        let mut tables = self.tables.write().await;
        let key = username.lookup_key();
        if tables.username_index.contains_key(&key) {
            return Err(Error::Conflict(format!("username `{username}` is taken")));
        }
        let user = User {
            id: UserId::new(),
            username: username.clone(),
            clan: None,
            rank_score: RankScore::ZERO,
            level: Level(1),
            trials_solved: 0,
            supporter: false,
            created_at: Timestamp::now(),
        };
        tables.username_index.insert(key, user.id);
        tables.users.insert(user.id, user.clone());
        Ok(user)
    }

    async fn user_by_username(&self, username: &str) -> Result<User> {
        let tables = self.tables.read().await;
        tables
            .username_index
            .get(&username.to_ascii_lowercase())
            .and_then(|id| tables.users.get(id))
            .cloned()
            .ok_or_else(|| Error::not_found("user", username))
    }

    async fn user_by_id(&self, id: UserId) -> Result<User> {
        let tables = self.tables.read().await;
        tables
            .users
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::not_found("user", id))
    }

    async fn global_rank(&self, id: UserId) -> Result<Option<u64>> {
        let tables = self.tables.read().await;
        let Some(user) = tables.users.get(&id) else {
            return Err(Error::not_found("user", id));
        };
        if user.rank_score.0 <= 0.0 {
            // Unranked until the user has scored anything. Showing "#1,204,915"
            // to someone who has solved nothing is discouraging and meaningless.
            return Ok(None);
        }
        let ahead = tables
            .users
            .values()
            .filter(|other| other.rank_score.0 > user.rank_score.0)
            .count();
        Ok(Some(ahead as u64 + 1))
    }

    async fn put_trial(&self, trial: &Trial) -> Result<()> {
        let mut tables = self.tables.write().await;
        tables
            .slug_index
            .insert(trial.slug.as_str().to_owned(), trial.id);
        tables.trials.insert(trial.id, trial.clone());
        Ok(())
    }

    async fn trial_by_slug(&self, slug: &TrialSlug) -> Result<Trial> {
        let tables = self.tables.read().await;
        tables
            .slug_index
            .get(slug.as_str())
            .and_then(|id| tables.trials.get(id))
            .cloned()
            .ok_or_else(|| Error::not_found("trial", slug))
    }

    async fn list_trials(&self, filter: &TrialFilter) -> Result<Vec<Trial>> {
        let tables = self.tables.read().await;
        let mut trials: Vec<Trial> = tables
            .trials
            .values()
            .filter(|t| t.is_listed())
            .filter(|t| filter.difficulty.is_none_or(|d| t.difficulty == d))
            .filter(|t| filter.topic.is_none_or(|topic| t.topics.contains(&topic)))
            .cloned()
            .collect();
        trials.sort_by(|a, b| a.slug.cmp(&b.slug));
        if filter.limit > 0 {
            trials.truncate(filter.limit);
        }
        Ok(trials)
    }

    async fn trial_user_state(&self, user_id: UserId, trial_id: TrialId) -> Result<TrialUserState> {
        let tables = self.tables.read().await;
        Ok(tables
            .trial_user_state
            .get(&(user_id, trial_id))
            .copied()
            .unwrap_or_default())
    }

    async fn reveal_solutions(&self, user_id: UserId, trial_id: TrialId) -> Result<()> {
        let mut tables = self.tables.write().await;
        tables
            .trial_user_state
            .entry((user_id, trial_id))
            .or_default()
            .revealed_solutions = true;
        Ok(())
    }

    async fn merge_checkpoint(
        &self,
        user_id: UserId,
        checkpoint: &Checkpoint,
    ) -> Result<MergeOutcome> {
        checkpoint.validate()?;
        let mut tables = self.tables.write().await;
        Ok(tables
            .progress
            .entry(user_id)
            .or_default()
            .merge(checkpoint))
    }

    async fn progress(&self, user_id: UserId) -> Result<ProgressState> {
        let tables = self.tables.read().await;
        Ok(tables.progress.get(&user_id).cloned().unwrap_or_default())
    }

    async fn create_submission(&self, new: NewSubmission) -> Result<(Submission, bool)> {
        let mut tables = self.tables.write().await;
        let idem_key = (new.user_id, new.idempotency_key.clone());
        if let Some(existing) = tables.idempotency.get(&idem_key).copied() {
            let submission = tables
                .submissions
                .get(&existing)
                .cloned()
                .ok_or_else(|| Error::Internal("dangling idempotency entry".into()))?;
            return Ok((submission, true));
        }

        let submission = Submission {
            id: SubmissionId::new(),
            job_id: JobId::new(),
            user_id: new.user_id,
            trial_id: new.trial_id,
            trial_version: new.trial_version,
            trial_package_cid: new.trial_package_cid,
            source_cid: new.source_cid,
            state: SubmissionState::Queued,
            created_at: Timestamp::now(),
            finished_at: None,
        };
        tables.idempotency.insert(idem_key, submission.id);
        tables.job_index.insert(submission.job_id, submission.id);
        tables.queue.push(submission.job_id);
        tables.submissions.insert(submission.id, submission.clone());
        Ok((submission, false))
    }

    async fn submission(&self, id: SubmissionId) -> Result<Submission> {
        let tables = self.tables.read().await;
        tables
            .submissions
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::not_found("submission", id))
    }

    async fn update_submission_state(&self, job_id: JobId, state: SubmissionState) -> Result<()> {
        let mut tables = self.tables.write().await;
        let submission_id = *tables
            .job_index
            .get(&job_id)
            .ok_or_else(|| Error::not_found("job", job_id))?;
        let submission = tables
            .submissions
            .get_mut(&submission_id)
            .ok_or_else(|| Error::Internal("dangling job index".into()))?;
        // A late heartbeat must never un-finish a finished submission.
        if !submission.state.is_terminal() {
            submission.state = state;
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
        let mut tables = self.tables.write().await;
        let take = (capacity as usize).min(tables.queue.len());
        let job_ids: Vec<JobId> = tables.queue.drain(..take).collect();

        let expires = Timestamp::now().plus_seconds(lease_seconds);
        let mut leased = Vec::with_capacity(job_ids.len());
        for job_id in job_ids {
            tables.leases.insert(job_id, worker_id.to_owned());
            let submission_id = tables.job_index[&job_id];
            let submission = tables.submissions[&submission_id].clone();
            leased.push(LeasedJob {
                job_id,
                source_cid: submission.source_cid.clone(),
                trial_package_cid: submission.trial_package_cid.clone(),
                trial_version: submission.trial_version,
                limits: ExecutionLimits::default(),
                // Invariant: hidden tests only ever reach a Trusted worker.
                may_receive_hidden_tests: trust_class.may_receive_hidden_tests(),
                lease_expires_at: expires,
            });
            if let Some(s) = tables.submissions.get_mut(&submission_id) {
                s.state = SubmissionState::Dispatched {
                    worker_id: worker_id.to_owned(),
                };
            }
        }
        Ok(leased)
    }

    async fn record_result(
        &self,
        job_id: JobId,
        worker_id: &str,
        trial_package_cid: &str,
        verdict: Verdict,
        _result_manifest_hash: &str,
    ) -> Result<SubmissionOutcome> {
        let mut tables = self.tables.write().await;

        let submission_id = *tables
            .job_index
            .get(&job_id)
            .ok_or_else(|| Error::not_found("job", job_id))?;
        let submission = tables.submissions[&submission_id].clone();

        if submission.trial_package_cid != trial_package_cid {
            return Err(Error::invalid(
                "trial_package_cid",
                "does not match the package leased for this submission",
            ));
        }

        if let Some(holder) = tables.leases.get(&job_id) {
            if holder != worker_id {
                return Err(Error::Forbidden("job is leased by another worker".into()));
            }
        }

        let trial = tables
            .trials
            .get(&submission.trial_id)
            .cloned()
            .ok_or_else(|| Error::not_found("trial", submission.trial_id))?;
        let state = tables
            .trial_user_state
            .get(&(submission.user_id, trial.id))
            .copied()
            .unwrap_or_default();
        let user = tables
            .users
            .get(&submission.user_id)
            .cloned()
            .ok_or_else(|| Error::not_found("user", submission.user_id))?;

        let context = Context {
            verdict,
            already_finished: submission.state.is_terminal(),
            existing_verdict: submission.state.verdict(),
            current_status: state.status,
            lifecycle: trial.lifecycle,
            difficulty: trial.difficulty,
            revealed_solutions: state.revealed_solutions,
            existing_solves: tables
                .solves
                .get(&submission.user_id)
                .cloned()
                .unwrap_or_default(),
            existing_experience: tables
                .experience
                .get(&submission.user_id)
                .copied()
                .unwrap_or(0),
        };
        let plan = finalize::plan(&context, user.trials_solved);

        if !plan.accepted {
            return Ok(SubmissionOutcome {
                accepted: false,
                recorded_verdict: plan.recorded_verdict,
                first_solve: false,
                user,
            });
        }

        // Apply. In PostgreSQL this whole block is one transaction.
        if let Some(s) = tables.submissions.get_mut(&submission_id) {
            s.state = SubmissionState::Finished {
                verdict: plan.recorded_verdict,
            };
            s.finished_at = Some(Timestamp::now());
        }
        tables.leases.remove(&job_id);

        let entry = tables
            .trial_user_state
            .entry((submission.user_id, trial.id))
            .or_default();
        entry.status = plan.new_status;
        if plan.increment_attempts {
            entry.attempts += 1;
        }
        let revealed = entry.revealed_solutions;

        if plan.first_solve {
            tables
                .solves
                .entry(submission.user_id)
                .or_default()
                .push(Solve {
                    difficulty: trial.difficulty,
                    lifecycle: trial.lifecycle,
                    revealed_solutions: revealed,
                });
        }
        if let Some(experience) = plan.new_experience {
            tables.experience.insert(submission.user_id, experience);
        }

        let user = tables
            .users
            .get_mut(&submission.user_id)
            .ok_or_else(|| Error::Internal("user vanished mid-transaction".into()))?;
        if plan.increment_trials_solved {
            user.trials_solved += 1;
        }
        if let Some(score) = plan.new_rank_score {
            user.rank_score = score;
        }
        if let Some(level) = plan.new_level {
            user.level = level;
        }
        let user = user.clone();

        Ok(SubmissionOutcome {
            accepted: true,
            recorded_verdict: plan.recorded_verdict,
            first_solve: plan.first_solve,
            user,
        })
    }

    async fn solves(&self, user_id: UserId) -> Result<Vec<Solve>> {
        let tables = self.tables.read().await;
        Ok(tables.solves.get(&user_id).cloned().unwrap_or_default())
    }

    async fn append_event(&self, event: &PlatformEvent) -> Result<()> {
        let mut tables = self.tables.write().await;
        tables.events.push(event.clone());
        Ok(())
    }

    async fn events(
        &self,
        since: Option<Timestamp>,
        kind: Option<EventKind>,
        limit: usize,
    ) -> Result<Vec<PlatformEvent>> {
        let tables = self.tables.read().await;
        let mut events: Vec<PlatformEvent> = tables
            .events
            .iter()
            .filter(|e| since.is_none_or(|s| e.occurred_at >= s))
            .filter(|e| kind.is_none_or(|k| e.kind == k))
            .cloned()
            .collect();
        events.sort_by_key(|e| std::cmp::Reverse(e.occurred_at));
        events.truncate(limit);
        Ok(events)
    }

    async fn create_clan(&self, clan: &Clan) -> Result<()> {
        let mut tables = self.tables.write().await;
        if tables.clans.contains_key(clan.tag.as_str()) {
            return Err(Error::Conflict(format!("clan tag `{}` is taken", clan.tag)));
        }
        tables
            .clans
            .insert(clan.tag.as_str().to_owned(), clan.clone());
        Ok(())
    }

    async fn add_clan_member(&self, tag: &ClanTag, user_id: UserId) -> Result<()> {
        let mut tables = self.tables.write().await;
        if !tables.clans.contains_key(tag.as_str()) {
            return Err(Error::not_found("clan", tag));
        }
        let members = tables
            .clan_members
            .entry(tag.as_str().to_owned())
            .or_default();
        if !members.contains(&user_id) {
            members.push(user_id);
        }
        if let Some(user) = tables.users.get_mut(&user_id) {
            user.clan = Some(tag.clone());
        }
        Ok(())
    }

    async fn clan_by_tag(&self, tag: &ClanTag) -> Result<Clan> {
        let tables = self.tables.read().await;
        tables
            .clans
            .get(tag.as_str())
            .cloned()
            .ok_or_else(|| Error::not_found("clan", tag))
    }

    async fn clan_members(&self, tag: &ClanTag) -> Result<Vec<Username>> {
        let tables = self.tables.read().await;
        if !tables.clans.contains_key(tag.as_str()) {
            return Err(Error::not_found("clan", tag));
        }
        let mut members: Vec<Username> = tables
            .clan_members
            .get(tag.as_str())
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| tables.users.get(id))
                    .map(|u| u.username.clone())
                    .collect()
            })
            .unwrap_or_default();
        members.sort_by_key(|u| u.lookup_key());
        Ok(members)
    }
}
