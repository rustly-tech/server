//! Submissions and judge verdicts.
//!
//! The critical rule encoded here: **infrastructure and cache failures are never
//! reported to a user as a compile or wrong-answer verdict.** `JE`, `IE` and `SE`
//! are a separate class, and a submission that ends in that class is retryable
//! and does not count as an attempt against the user.

use rustly_common::{Id, Timestamp};
use serde::{Deserialize, Serialize};

use crate::trial::TrialId;
use crate::user::UserId;

/// Type tag for a submission id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmissionTag;
/// A submission identifier.
pub type SubmissionId = Id<SubmissionTag>;

/// Type tag for a judge job id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobTag;
/// An immutable judge job identifier, minted once per submission attempt.
pub type JobId = Id<JobTag>;

/// A judge verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Verdict {
    /// Accepted.
    #[serde(rename = "AC")]
    Accepted,
    /// Compile error in the user's code.
    #[serde(rename = "CE")]
    CompileError,
    /// Wrong answer.
    #[serde(rename = "WA")]
    WrongAnswer,
    /// Time limit exceeded.
    #[serde(rename = "TLE")]
    TimeLimitExceeded,
    /// Memory limit exceeded.
    #[serde(rename = "MLE")]
    MemoryLimitExceeded,
    /// Output limit exceeded.
    #[serde(rename = "OLE")]
    OutputLimitExceeded,
    /// Runtime error in the user's program.
    #[serde(rename = "RTE")]
    RuntimeError,
    /// Judge error: the judge itself failed (bad problem package, checker crash).
    #[serde(rename = "JE")]
    JudgeError,
    /// Internal error: Rustly infrastructure failed (queue, storage, cache).
    #[serde(rename = "IE")]
    InternalError,
    /// Security event: the submission tripped a sandbox policy.
    #[serde(rename = "SE")]
    SecurityEvent,
}

/// Who a verdict is "about".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerdictClass {
    /// The user's submission was correct.
    Accepted,
    /// The user's submission was incorrect or exceeded a limit. Their problem.
    UserFault,
    /// Rustly or the judge failed. Not the user's problem; retryable.
    SystemFault,
}

impl Verdict {
    /// Every verdict, in the order they are documented.
    pub const ALL: [Verdict; 10] = [
        Self::Accepted,
        Self::CompileError,
        Self::WrongAnswer,
        Self::TimeLimitExceeded,
        Self::MemoryLimitExceeded,
        Self::OutputLimitExceeded,
        Self::RuntimeError,
        Self::JudgeError,
        Self::InternalError,
        Self::SecurityEvent,
    ];

    /// The short code used on the wire and in the UI.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Accepted => "AC",
            Self::CompileError => "CE",
            Self::WrongAnswer => "WA",
            Self::TimeLimitExceeded => "TLE",
            Self::MemoryLimitExceeded => "MLE",
            Self::OutputLimitExceeded => "OLE",
            Self::RuntimeError => "RTE",
            Self::JudgeError => "JE",
            Self::InternalError => "IE",
            Self::SecurityEvent => "SE",
        }
    }

    /// Classify who the verdict is about.
    pub const fn class(self) -> VerdictClass {
        match self {
            Self::Accepted => VerdictClass::Accepted,
            Self::CompileError
            | Self::WrongAnswer
            | Self::TimeLimitExceeded
            | Self::MemoryLimitExceeded
            | Self::OutputLimitExceeded
            | Self::RuntimeError
            | Self::SecurityEvent => VerdictClass::UserFault,
            Self::JudgeError | Self::InternalError => VerdictClass::SystemFault,
        }
    }

    /// Whether the submission may be transparently retried by the platform.
    pub const fn is_retryable(self) -> bool {
        matches!(self.class(), VerdictClass::SystemFault)
    }

    /// Whether this verdict counts as a user attempt on the Trial.
    ///
    /// System faults do not: a queue outage must not turn a Trial from
    /// `Unsolved` into `Attempted`.
    pub const fn counts_as_attempt(self) -> bool {
        !self.is_retryable()
    }
}

/// The lifecycle of a submission, as streamed to the client over SSE.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SubmissionState {
    /// Accepted by the API, job minted, not yet leased.
    Queued,
    /// Leased by a worker.
    Dispatched {
        /// Opaque worker identifier, for support and metrics.
        worker_id: String,
    },
    /// Compiling in the compile sandbox.
    Compiling,
    /// Running tests in the runtime sandbox.
    Running {
        /// Tests completed so far.
        completed: u32,
        /// Total tests in the dispatched set.
        total: u32,
    },
    /// Terminal.
    Finished {
        /// The verdict.
        verdict: Verdict,
    },
}

impl SubmissionState {
    /// Whether this is a terminal state.
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Finished { .. })
    }

    /// The terminal verdict, if any.
    pub const fn verdict(&self) -> Option<Verdict> {
        match self {
            Self::Finished { verdict } => Some(*verdict),
            _ => None,
        }
    }
}

/// A submission record.
///
/// The source itself is **not** stored here: it lives in the data plane,
/// addressed by [`Submission::source_cid`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Submission {
    /// Stable identifier.
    pub id: SubmissionId,
    /// Immutable job identifier for the judge.
    pub job_id: JobId,
    /// Submitting user.
    pub user_id: UserId,
    /// Trial being attempted.
    pub trial_id: TrialId,
    /// Trial content version this submission was judged against.
    pub trial_version: u32,
    /// Immutable evaluation package selected when the submission was created.
    pub trial_package_cid: String,
    /// BLAKE3 CID of the submitted source in the data plane.
    pub source_cid: String,
    /// Current state.
    pub state: SubmissionState,
    /// Creation time.
    pub created_at: Timestamp,
    /// Terminal time, if finished.
    pub finished_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_ten_verdicts_have_distinct_stable_codes() {
        let codes: Vec<_> = Verdict::ALL.iter().map(|v| v.code()).collect();
        assert_eq!(
            codes,
            ["AC", "CE", "WA", "TLE", "MLE", "OLE", "RTE", "JE", "IE", "SE"]
        );
        let unique: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(unique.len(), codes.len());
    }

    #[test]
    fn wire_form_matches_the_short_code() {
        for v in Verdict::ALL {
            assert_eq!(
                serde_json::to_string(&v).unwrap(),
                format!("\"{}\"", v.code())
            );
        }
    }

    #[test]
    fn infrastructure_failures_are_never_user_fault() {
        for v in [Verdict::JudgeError, Verdict::InternalError] {
            assert_eq!(v.class(), VerdictClass::SystemFault, "{}", v.code());
            assert!(v.is_retryable(), "{} must be retryable", v.code());
            assert!(
                !v.counts_as_attempt(),
                "{} must not count as an attempt",
                v.code()
            );
        }
    }

    #[test]
    fn user_faults_are_not_retryable_and_do_count() {
        for v in [
            Verdict::CompileError,
            Verdict::WrongAnswer,
            Verdict::TimeLimitExceeded,
            Verdict::MemoryLimitExceeded,
            Verdict::OutputLimitExceeded,
            Verdict::RuntimeError,
            Verdict::SecurityEvent,
        ] {
            assert_eq!(v.class(), VerdictClass::UserFault, "{}", v.code());
            assert!(!v.is_retryable(), "{}", v.code());
            assert!(v.counts_as_attempt(), "{}", v.code());
        }
    }

    #[test]
    fn accepted_is_its_own_class() {
        assert_eq!(Verdict::Accepted.class(), VerdictClass::Accepted);
        assert!(!Verdict::Accepted.is_retryable());
        assert!(Verdict::Accepted.counts_as_attempt());
    }

    #[test]
    fn only_finished_is_terminal() {
        assert!(!SubmissionState::Queued.is_terminal());
        assert!(!SubmissionState::Compiling.is_terminal());
        assert!(!SubmissionState::Running {
            completed: 1,
            total: 4
        }
        .is_terminal());
        let done = SubmissionState::Finished {
            verdict: Verdict::Accepted,
        };
        assert!(done.is_terminal());
        assert_eq!(done.verdict(), Some(Verdict::Accepted));
    }

    #[test]
    fn state_is_tagged_on_the_wire() {
        let json = serde_json::to_value(SubmissionState::Running {
            completed: 2,
            total: 5,
        })
        .unwrap();
        assert_eq!(json["state"], "running");
        assert_eq!(json["completed"], 2);
        assert_eq!(json["total"], 5);
    }
}
