//! The Rustly product domain.
//!
//! Pure types and pure rules. No I/O, no async, no database, no HTTP. Everything
//! here is directly testable, and every product rule that matters
//! (milestone display, verdict classification, trial lifecycle) lives here
//! rather than being smeared across handlers.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod clan;
pub mod event;
pub mod milestone;
pub mod submission;
pub mod trial;
pub mod user;

pub use clan::{Clan, ClanId, ClanMembership, ClanTag};
pub use event::{EventId, EventKind, PlatformEvent};
pub use milestone::TrialMilestone;
pub use submission::{JobId, Submission, SubmissionId, SubmissionState, Verdict, VerdictClass};
pub use trial::{Difficulty, Trial, TrialId, TrialLifecycle, TrialSlug, TrialStatus, TrialTopic};
pub use user::{Level, PublicProfile, RankScore, User, UserId, Username};
