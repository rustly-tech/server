//! A backend-agnostic conformance suite.
//!
//! Every [`MetadataStore`] implementation must pass exactly the same tests. That
//! is what makes it legitimate for the API integration tests to run against the
//! in-memory backend: the behaviour they exercise is proven identical to the
//! PostgreSQL backend by this suite.
//!
//! Enabled by the `testing` feature. The in-memory backend runs it on every
//! `cargo test`; the PostgreSQL backend runs it in CI against a real server.

use rustly_common::Timestamp;
use rustly_domain::clan::ClanId;
use rustly_domain::event::EventId;
use rustly_domain::trial::TrialId;
use rustly_domain::{
    Clan, ClanTag, Difficulty, EventKind, PlatformEvent, Trial, TrialLifecycle, TrialSlug,
    TrialStatus, TrialTopic, Username, Verdict,
};
use rustly_progress::{Checkpoint, Completion, Entry, CHECKPOINT_FORMAT_VERSION};
use rustly_protocol::broker::TrustClass;

use crate::store::{MetadataStore, NewSubmission, TrialFilter};

/// `len` random hex characters.
///
/// Taken from the *tail* of a v4 UUID, which is random. The head of a v7 UUID is
/// a millisecond timestamp, so a truncated v7 collides for every call inside the
/// same few hundred milliseconds - which is exactly what a fast test suite does.
fn token(len: usize) -> String {
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    uuid[uuid.len() - len..].to_owned()
}

/// A distinct identifier per call, so a shared database can host repeated runs.
fn unique(prefix: &str) -> String {
    format!("{prefix}{}", token(10))
}

fn trial(slug: &str, difficulty: Difficulty, lifecycle: TrialLifecycle) -> Trial {
    Trial {
        id: TrialId::new(),
        slug: TrialSlug::parse(slug).expect("test slug must be valid"),
        title: format!("Trial {slug}"),
        difficulty,
        topics: vec![TrialTopic::Ownership],
        lifecycle,
        version: 1,
        content_cid: "b3:0000000000000000".into(),
        published_at: Timestamp::now(),
    }
}

/// Run the full suite against `store`.
///
/// # Panics
///
/// Panics on the first behavioural difference from the specification, with a
/// message naming the rule that was violated.
pub async fn run_suite<S: MetadataStore + ?Sized>(store: &S) {
    store.ping().await.expect("store must be reachable");

    users_and_ranking(store).await;
    trials_and_listing(store).await;
    progress_is_idempotent(store).await;
    submission_idempotency(store).await;
    hidden_tests_reach_only_trusted_workers(store).await;
    accepted_result_is_idempotent_and_updates_rank(store).await;
    system_faults_do_not_touch_user_state(store).await;
    events_window_and_ordering(store).await;
    clans(store).await;
}

async fn users_and_ranking<S: MetadataStore + ?Sized>(store: &S) {
    let name = Username::parse(&unique("user")).unwrap();
    let user = store.create_user(&name).await.expect("create_user");
    assert_eq!(user.trials_solved, 0);
    assert_eq!(user.level.0, 1);

    // Case-insensitive lookup.
    let upper = store
        .user_by_username(&name.as_str().to_uppercase())
        .await
        .unwrap();
    assert_eq!(
        upper.id, user.id,
        "username lookup must be case-insensitive"
    );

    // Duplicate usernames are a conflict, not a second account.
    let err = store.create_user(&name).await.unwrap_err();
    assert_eq!(err.code(), "conflict", "duplicate username must conflict");

    // Unranked users have no global rank rather than a discouraging huge number.
    assert_eq!(store.global_rank(user.id).await.unwrap(), None);

    assert_eq!(
        store
            .user_by_username("definitely-not-a-user-xyz")
            .await
            .unwrap_err()
            .code(),
        "not_found"
    );
}

async fn trials_and_listing<S: MetadataStore + ?Sized>(store: &S) {
    let listed = trial(
        &unique("t-listed-"),
        Difficulty::Medium,
        TrialLifecycle::Verified,
    );
    let draft = trial(
        &unique("t-draft-"),
        Difficulty::Medium,
        TrialLifecycle::Draft,
    );
    store.put_trial(&listed).await.unwrap();
    store.put_trial(&draft).await.unwrap();

    let fetched = store.trial_by_slug(&listed.slug).await.unwrap();
    assert_eq!(fetched.id, listed.id);
    assert_eq!(fetched.topics, vec![TrialTopic::Ownership]);

    let all = store.list_trials(&TrialFilter::default()).await.unwrap();
    assert!(
        all.iter().any(|t| t.id == listed.id),
        "verified Trials are listed"
    );
    assert!(
        !all.iter().any(|t| t.id == draft.id),
        "draft Trials must never be listed"
    );

    let filtered = store
        .list_trials(&TrialFilter {
            difficulty: Some(Difficulty::Expert),
            topic: None,
            limit: 0,
        })
        .await
        .unwrap();
    assert!(
        !filtered.iter().any(|t| t.id == listed.id),
        "difficulty filter must apply"
    );

    let by_topic = store
        .list_trials(&TrialFilter {
            difficulty: None,
            topic: Some(TrialTopic::Concurrency),
            limit: 0,
        })
        .await
        .unwrap();
    assert!(
        !by_topic.iter().any(|t| t.id == listed.id),
        "topic filter must apply"
    );
}

async fn progress_is_idempotent<S: MetadataStore + ?Sized>(store: &S) {
    let user = store
        .create_user(&Username::parse(&unique("prog")).unwrap())
        .await
        .unwrap();
    let key = "learn/ownership/move-semantics";
    let checkpoint = Checkpoint {
        format_version: CHECKPOINT_FORMAT_VERSION,
        device: "device-a".into(),
        entries: vec![Entry {
            key: key.into(),
            revision: 4,
            completion: Completion::Completed,
            recorded_at: Timestamp::now(),
        }],
    };

    let first = store.merge_checkpoint(user.id, &checkpoint).await.unwrap();
    assert_eq!(first.inserted, 1);

    let second = store.merge_checkpoint(user.id, &checkpoint).await.unwrap();
    assert!(!second.changed(), "replaying a checkpoint must be a no-op");
    assert_eq!(second.unchanged, 1);

    // A stale device cannot demote progress.
    let stale = Checkpoint {
        format_version: CHECKPOINT_FORMAT_VERSION,
        device: "old-laptop".into(),
        entries: vec![Entry {
            key: key.into(),
            revision: 1,
            completion: Completion::NotStarted,
            recorded_at: Timestamp::now().plus_seconds(-99_999),
        }],
    };
    store.merge_checkpoint(user.id, &stale).await.unwrap();

    let state = store.progress(user.id).await.unwrap();
    assert_eq!(state.entries[key].completion, Completion::Completed);
    assert_eq!(state.entries[key].revision, 4);

    // Malformed batches are rejected before they reach storage.
    let bad = Checkpoint {
        format_version: 999,
        ..checkpoint.clone()
    };
    assert_eq!(
        store
            .merge_checkpoint(user.id, &bad)
            .await
            .unwrap_err()
            .code(),
        "invalid_request"
    );
}

async fn submission_idempotency<S: MetadataStore + ?Sized>(store: &S) {
    let user = store
        .create_user(&Username::parse(&unique("subm")).unwrap())
        .await
        .unwrap();
    let t = trial(
        &unique("t-idem-"),
        Difficulty::Easy,
        TrialLifecycle::Verified,
    );
    store.put_trial(&t).await.unwrap();

    let new = NewSubmission {
        user_id: user.id,
        trial_id: t.id,
        trial_version: t.version,
        source_cid: "b3:source".into(),
        idempotency_key: unique("idem-"),
    };

    let (first, replayed) = store.create_submission(new.clone()).await.unwrap();
    assert!(!replayed);
    assert!(!first.state.is_terminal());

    let (again, replayed) = store.create_submission(new).await.unwrap();
    assert!(
        replayed,
        "the same idempotency key must replay, not create a second submission"
    );
    assert_eq!(again.id, first.id);
    assert_eq!(again.job_id, first.job_id, "the job id is immutable");

    assert_eq!(store.submission(first.id).await.unwrap().id, first.id);
}

async fn hidden_tests_reach_only_trusted_workers<S: MetadataStore + ?Sized>(store: &S) {
    let user = store
        .create_user(&Username::parse(&unique("lease")).unwrap())
        .await
        .unwrap();
    let t = trial(
        &unique("t-lease-"),
        Difficulty::Easy,
        TrialLifecycle::Verified,
    );
    store.put_trial(&t).await.unwrap();

    for class in [
        TrustClass::Volunteer,
        TrustClass::Community,
        TrustClass::Trusted,
    ] {
        store
            .create_submission(NewSubmission {
                user_id: user.id,
                trial_id: t.id,
                trial_version: t.version,
                source_cid: "b3:source".into(),
                idempotency_key: unique("idem-"),
            })
            .await
            .unwrap();

        let leased = store.lease_jobs(&unique("w-"), class, 1, 30).await.unwrap();
        assert_eq!(
            leased.len(),
            1,
            "one queued job should be leased for {class:?}"
        );
        let job = &leased[0];
        assert_eq!(
            job.may_receive_hidden_tests,
            class == TrustClass::Trusted,
            "hidden tests must reach only Trusted workers, not {class:?}"
        );
        assert!(
            !job.source_cid.is_empty(),
            "a lease carries a CID, never source bytes"
        );
        assert!(job.limits.fuel > 0 && job.limits.memory_bytes > 0);
    }
}

async fn accepted_result_is_idempotent_and_updates_rank<S: MetadataStore + ?Sized>(store: &S) {
    let user = store
        .create_user(&Username::parse(&unique("solver")).unwrap())
        .await
        .unwrap();
    let t = trial(
        &unique("t-solve-"),
        Difficulty::Medium,
        TrialLifecycle::Verified,
    );
    store.put_trial(&t).await.unwrap();

    let (submission, _) = store
        .create_submission(NewSubmission {
            user_id: user.id,
            trial_id: t.id,
            trial_version: t.version,
            source_cid: "b3:source".into(),
            idempotency_key: unique("idem-"),
        })
        .await
        .unwrap();

    let worker = unique("w-");
    let leased = store
        .lease_jobs(&worker, TrustClass::Trusted, 8, 30)
        .await
        .unwrap();
    assert!(leased.iter().any(|j| j.job_id == submission.job_id));

    let outcome = store
        .record_result(submission.job_id, &worker, Verdict::Accepted, "b3:manifest")
        .await
        .unwrap();
    assert!(outcome.accepted);
    assert!(outcome.first_solve);
    assert_eq!(outcome.user.trials_solved, 1);
    assert!(
        outcome.user.rank_score.0 > 0.0,
        "a verified solve must move the provisional rank"
    );

    // Idempotent: a duplicate report changes nothing.
    let duplicate = store
        .record_result(
            submission.job_id,
            &worker,
            Verdict::WrongAnswer,
            "b3:manifest",
        )
        .await
        .unwrap();
    assert!(
        !duplicate.accepted,
        "a second report must not finalise again"
    );
    assert_eq!(duplicate.recorded_verdict, Verdict::Accepted);
    assert_eq!(
        duplicate.user.trials_solved, 1,
        "trials_solved must not double-count"
    );

    let finished = store.submission(submission.id).await.unwrap();
    assert_eq!(finished.state.verdict(), Some(Verdict::Accepted));
    assert!(finished.state.is_terminal());

    let state = store.trial_user_state(user.id, t.id).await.unwrap();
    assert_eq!(state.status, TrialStatus::Solved);

    assert_eq!(store.solves(user.id).await.unwrap().len(), 1);
    assert!(store.global_rank(user.id).await.unwrap().is_some());
}

async fn system_faults_do_not_touch_user_state<S: MetadataStore + ?Sized>(store: &S) {
    let user = store
        .create_user(&Username::parse(&unique("infra")).unwrap())
        .await
        .unwrap();
    let t = trial(
        &unique("t-infra-"),
        Difficulty::Hard,
        TrialLifecycle::Verified,
    );
    store.put_trial(&t).await.unwrap();

    let (submission, _) = store
        .create_submission(NewSubmission {
            user_id: user.id,
            trial_id: t.id,
            trial_version: t.version,
            source_cid: "b3:source".into(),
            idempotency_key: unique("idem-"),
        })
        .await
        .unwrap();

    let worker = unique("w-");
    store
        .lease_jobs(&worker, TrustClass::Trusted, 8, 30)
        .await
        .unwrap();
    let outcome = store
        .record_result(
            submission.job_id,
            &worker,
            Verdict::InternalError,
            "b3:manifest",
        )
        .await
        .unwrap();

    assert!(outcome.accepted, "the verdict is recorded for operators");
    assert_eq!(outcome.user.trials_solved, 0);
    let state = store.trial_user_state(user.id, t.id).await.unwrap();
    assert_eq!(
        state.status,
        TrialStatus::Unsolved,
        "an infrastructure failure must not mark a Trial attempted"
    );
    assert_eq!(state.attempts, 0);
}

async fn events_window_and_ordering<S: MetadataStore + ?Sized>(store: &S) {
    let marker = unique("evt-");
    let now = Timestamp::now();
    for (offset, summary) in [(-10_i64, "newer"), (-1_000, "older"), (-200_000, "ancient")] {
        store
            .append_event(&PlatformEvent {
                id: EventId::new(),
                kind: EventKind::TrialPublished,
                summary: format!("{marker} {summary}"),
                href: None,
                subject: None,
                occurred_at: now.plus_seconds(offset),
            })
            .await
            .unwrap();
    }

    let recent = store
        .events(Some(now.plus_seconds(-86_400)), None, 100)
        .await
        .unwrap();
    let mine: Vec<_> = recent
        .iter()
        .filter(|e| e.summary.starts_with(&marker))
        .collect();
    assert_eq!(
        mine.len(),
        2,
        "the 24h window must exclude the ancient event"
    );
    assert!(
        mine[0].summary.ends_with("newer"),
        "events must be newest first"
    );

    let archive = store
        .events(None, Some(EventKind::TrialPublished), 500)
        .await
        .unwrap();
    assert!(
        archive
            .iter()
            .filter(|e| e.summary.starts_with(&marker))
            .count()
            == 3
    );

    let other_kind = store
        .events(None, Some(EventKind::EventStarted), 500)
        .await
        .unwrap();
    assert!(
        !other_kind.iter().any(|e| e.summary.starts_with(&marker)),
        "kind filter must apply"
    );
}

async fn clans<S: MetadataStore + ?Sized>(store: &S) {
    let owner = store
        .create_user(&Username::parse(&unique("owner")).unwrap())
        .await
        .unwrap();
    let member = store
        .create_user(&Username::parse(&unique("member")).unwrap())
        .await
        .unwrap();

    // Six random uppercase hex characters: inside the 2-6 character tag rule and
    // unique enough to re-run against a shared database.
    let tag = ClanTag::parse(&token(6).to_uppercase()).expect("generated clan tag must be valid");

    let clan = Clan {
        id: ClanId::new(),
        tag: tag.clone(),
        name: "Test Clan".into(),
        blurb: Some("A clan for the conformance suite".into()),
        owner: owner.id,
        created_at: Timestamp::now(),
    };
    store.create_clan(&clan).await.unwrap();
    assert_eq!(
        store.create_clan(&clan).await.unwrap_err().code(),
        "conflict"
    );

    store.add_clan_member(&tag, owner.id).await.unwrap();
    store.add_clan_member(&tag, member.id).await.unwrap();
    // Adding twice is not an error and does not duplicate the member.
    store.add_clan_member(&tag, member.id).await.unwrap();

    let fetched = store.clan_by_tag(&tag).await.unwrap();
    assert_eq!(fetched.name, "Test Clan");
    assert_eq!(fetched.owner, owner.id);

    let members = store.clan_members(&tag).await.unwrap();
    assert_eq!(members.len(), 2, "membership must not duplicate");

    let profile_owner = store.user_by_id(owner.id).await.unwrap();
    assert_eq!(
        profile_owner.clan.as_ref(),
        Some(&tag),
        "the tag appears on the profile"
    );

    let missing = ClanTag::parse("ZZZZZZ").unwrap();
    assert_eq!(
        store.clan_by_tag(&missing).await.unwrap_err().code(),
        "not_found"
    );
    assert_eq!(
        store.clan_members(&missing).await.unwrap_err().code(),
        "not_found"
    );
}
