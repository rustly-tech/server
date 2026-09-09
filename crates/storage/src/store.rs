//! The [`MetadataStore`] trait: the only persistence surface the rest of the
//! control plane knows about.

use async_trait::async_trait;
use rustly_common::{Result, Timestamp};
use rustly_domain::submission::JobId;
use rustly_domain::trial::TrialId;
use rustly_domain::user::UserId;
use rustly_domain::{
    Clan, ClanTag, Difficulty, EventKind, PlatformEvent, Submission, SubmissionId, SubmissionState,
    Trial, TrialSlug, TrialStatus, TrialTopic, User, Username,
};
use rustly_progress::{Checkpoint, MergeOutcome, ProgressState};
use rustly_protocol::broker::{ExecutionLimits, TrustClass};
use rustly_ranking::Solve;

/// Filter for listing Trials.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrialFilter {
    /// Restrict to one difficulty.
    pub difficulty: Option<Difficulty>,
    /// Restrict to one topic.
    pub topic: Option<TrialTopic>,
    /// Maximum results.
    pub limit: usize,
}

/// A user's relationship to one Trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrialUserState {
    /// Displayed status.
    pub status: TrialStatus,
    /// Number of attempts that counted (system faults excluded).
    pub attempts: u32,
    /// Whether the user revealed published solutions before solving.
    pub revealed_solutions: bool,
}

impl Default for TrialUserState {
    fn default() -> Self {
        Self {
            status: TrialStatus::Unsolved,
            attempts: 0,
            revealed_solutions: false,
        }
    }
}

/// A request to create a submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSubmission {
    /// Submitting user.
    pub user_id: UserId,
    /// Trial being attempted.
    pub trial_id: TrialId,
    /// Trial content version at submission time.
    pub trial_version: u32,
    /// Trial package CID selected at submission time.
    pub trial_package_cid: String,
    /// CID of the submitted source in the data plane.
    pub source_cid: String,
    /// Client idempotency key, unique per user.
    pub idempotency_key: String,
}

/// A job leased to a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeasedJob {
    /// Job identifier.
    pub job_id: JobId,
    /// CID of the source to judge.
    pub source_cid: String,
    /// CID of the Trial package.
    pub trial_package_cid: String,
    /// Trial content version.
    pub trial_version: u32,
    /// Limits the worker must enforce.
    pub limits: ExecutionLimits,
    /// Whether hidden tests are included, decided from the worker's trust class.
    pub may_receive_hidden_tests: bool,
    /// Lease expiry.
    pub lease_expires_at: Timestamp,
}

/// What recording a judge result actually did.
#[derive(Debug, Clone, PartialEq)]
pub struct SubmissionOutcome {
    /// Whether this report finalised the submission. `false` on a duplicate.
    pub accepted: bool,
    /// The verdict now recorded.
    pub recorded_verdict: rustly_domain::Verdict,
    /// Whether this was the user's first accepted solve of the Trial.
    pub first_solve: bool,
    /// The user record after any ranking update.
    pub user: User,
}

/// The persistence surface of the control plane.
#[async_trait]
pub trait MetadataStore: Send + Sync + 'static {
    /// Confirm the backend is reachable. Used by the readiness endpoint.
    async fn ping(&self) -> Result<()>;

    /// Create a user.
    async fn create_user(&self, username: &Username) -> Result<User>;

    /// Look up a user by username, case-insensitively.
    async fn user_by_username(&self, username: &str) -> Result<User>;

    /// Look up a user by id.
    async fn user_by_id(&self, id: UserId) -> Result<User>;

    /// The user's 1-based position in the global ordering, or `None` if unranked.
    async fn global_rank(&self, id: UserId) -> Result<Option<u64>>;

    /// Insert or replace a Trial's metadata.
    async fn put_trial(&self, trial: &Trial) -> Result<()>;

    /// Fetch a Trial by slug.
    async fn trial_by_slug(&self, slug: &TrialSlug) -> Result<Trial>;

    /// List listed Trials matching `filter`.
    async fn list_trials(&self, filter: &TrialFilter) -> Result<Vec<Trial>>;

    /// A user's state for one Trial.
    async fn trial_user_state(&self, user_id: UserId, trial_id: TrialId) -> Result<TrialUserState>;

    /// Record that a user revealed published solutions, forfeiting rank credit.
    async fn reveal_solutions(&self, user_id: UserId, trial_id: TrialId) -> Result<()>;

    /// Merge a progress checkpoint. Idempotent.
    async fn merge_checkpoint(
        &self,
        user_id: UserId,
        checkpoint: &Checkpoint,
    ) -> Result<MergeOutcome>;

    /// Read merged progress.
    async fn progress(&self, user_id: UserId) -> Result<ProgressState>;

    /// Create a submission, or replay the existing one for the same
    /// idempotency key. The `bool` is `true` when this was a replay.
    async fn create_submission(&self, new: NewSubmission) -> Result<(Submission, bool)>;

    /// Fetch a submission.
    async fn submission(&self, id: SubmissionId) -> Result<Submission>;

    /// Update a non-terminal submission state, e.g. from a worker heartbeat.
    ///
    /// Must not overwrite a terminal state: a late heartbeat cannot un-finish a
    /// finished submission.
    async fn update_submission_state(&self, job_id: JobId, state: SubmissionState) -> Result<()>;

    /// Lease up to `capacity` queued jobs for a worker.
    async fn lease_jobs(
        &self,
        worker_id: &str,
        trust_class: TrustClass,
        capacity: u32,
        lease_seconds: i64,
    ) -> Result<Vec<LeasedJob>>;

    /// Record a judge result. Idempotent per job.
    async fn record_result(
        &self,
        job_id: JobId,
        worker_id: &str,
        trial_package_cid: &str,
        verdict: rustly_domain::Verdict,
        result_manifest_hash: &str,
    ) -> Result<SubmissionOutcome>;

    /// Every counting solve for a user, as input to the ranking model.
    async fn solves(&self, user_id: UserId) -> Result<Vec<Solve>>;

    /// Append a platform event.
    async fn append_event(&self, event: &PlatformEvent) -> Result<()>;

    /// Events at or after `since`, newest first, optionally filtered by kind.
    async fn events(
        &self,
        since: Option<Timestamp>,
        kind: Option<EventKind>,
        limit: usize,
    ) -> Result<Vec<PlatformEvent>>;

    /// Create a clan.
    async fn create_clan(&self, clan: &Clan) -> Result<()>;

    /// Add a member to a clan and stamp the tag onto their profile.
    async fn add_clan_member(&self, tag: &ClanTag, user_id: UserId) -> Result<()>;

    /// Fetch a clan by tag.
    async fn clan_by_tag(&self, tag: &ClanTag) -> Result<Clan>;

    /// List a clan's members.
    async fn clan_members(&self, tag: &ClanTag) -> Result<Vec<Username>>;
}
