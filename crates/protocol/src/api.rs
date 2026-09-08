//! Public HTTP API contracts (`/api/v1`).

use rustly_common::Timestamp;
use rustly_domain::{
    Difficulty, Level, PublicProfile, RankScore, SubmissionId, SubmissionState, TrialLifecycle,
    TrialSlug, TrialStatus, TrialTopic, Username,
};
use rustly_progress::{Checkpoint, Completion, MergeOutcome};
use serde::{Deserialize, Serialize};

/// Response of `GET /api/version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionResponse {
    /// Major API version.
    pub api_version: u32,
    /// Judge broker protocol version.
    pub broker_protocol_version: u32,
    /// Build identifier (git SHA when built in CI).
    pub build: String,
    /// Id of the ranking model currently in use.
    pub ranking_model: String,
    /// Whether that ranking model is provisional. Surfaced so the UI can label
    /// the number honestly rather than implying it is a validated rating.
    pub ranking_provisional: bool,
}

/// Response of `GET /health` and `GET /ready`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    /// `"ok"` or `"degraded"`.
    pub status: &'static str,
    /// Per-dependency readiness. Empty for the liveness endpoint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<DependencyCheck>,
}

/// One dependency's readiness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyCheck {
    /// Dependency name, e.g. `"metadata_store"`.
    pub name: String,
    /// Whether it responded.
    pub healthy: bool,
    /// Detail, omitted when healthy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A uniform error body. Every non-2xx response has this shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Stable machine-readable code, e.g. `"not_found"`.
    pub code: String,
    /// Human-readable message. Never contains internal detail.
    pub message: String,
    /// Correlation id, also returned in the `x-request-id` header.
    pub request_id: String,
}

/// Response of `GET /api/v1/users/{username}`.
///
/// A thin envelope around [`PublicProfile`] so we can add response-level
/// metadata later without changing the profile shape itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileResponse {
    /// The minimal public profile.
    pub profile: PublicProfile,
}

/// Response of `GET /api/v1/me`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeResponse {
    /// Username.
    pub username: Username,
    /// Provisional rank score.
    pub rank: RankScore,
    /// Level.
    pub level: Level,
    /// Distinct Trials solved.
    pub trials_solved: u32,
    /// Whether a supporter entitlement is active. Cosmetic only.
    pub supporter: bool,
}

/// Trial metadata as returned by the API.
///
/// Statement, starter code, and public tests are not here: they are static
/// content fetched by CID, which is what keeps ordinary Trial reads off the API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrialSummary {
    /// URL slug.
    pub slug: TrialSlug,
    /// Human title.
    pub title: String,
    /// Difficulty tier.
    pub difficulty: Difficulty,
    /// Topic tags.
    pub topics: Vec<TrialTopic>,
    /// Editorial state.
    pub lifecycle: TrialLifecycle,
    /// Content version the verdict would apply to.
    pub version: u32,
    /// BLAKE3 CID of the immutable Trial package.
    pub content_cid: String,
    /// Caller's status for this Trial. `Unsolved` for anonymous callers.
    pub status: TrialStatus,
}

/// Response of `GET /api/v1/trials`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrialListResponse {
    /// Matching Trials.
    pub trials: Vec<TrialSummary>,
}

/// Query parameters for `GET /api/v1/trials`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrialQuery {
    /// Filter by difficulty.
    #[serde(default)]
    pub difficulty: Option<Difficulty>,
    /// Filter by topic.
    #[serde(default)]
    pub topic: Option<TrialTopic>,
    /// Filter by the caller's status.
    #[serde(default)]
    pub status: Option<TrialStatus>,
}

/// Request body of `POST /api/v1/progress/checkpoints`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckpointRequest {
    /// The batch to merge.
    #[serde(flatten)]
    pub checkpoint: Checkpoint,
}

/// Response of `POST /api/v1/progress/checkpoints`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointResponse {
    /// Entries created.
    pub inserted: usize,
    /// Entries whose merged value changed.
    pub updated: usize,
    /// Entries that carried nothing new. A retry reports everything here.
    pub unchanged: usize,
}

impl From<MergeOutcome> for CheckpointResponse {
    fn from(outcome: MergeOutcome) -> Self {
        Self {
            inserted: outcome.inserted,
            updated: outcome.updated,
            unchanged: outcome.unchanged,
        }
    }
}

/// Response of `GET /api/v1/progress`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressResponse {
    /// Merged completion state, keyed by content key.
    pub entries: std::collections::BTreeMap<String, Completion>,
    /// Number of units at `Completed` or better.
    pub completed: usize,
}

/// Request body of `POST /api/v1/submissions`.
///
/// The source is **not** in the body. The client writes it to the data plane
/// first and submits the resulting CID, so submissions of any size never pass
/// through the control plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSubmissionRequest {
    /// Trial slug being attempted.
    pub trial: String,
    /// BLAKE3 CID of the submitted source.
    pub source_cid: String,
    /// Client-generated idempotency key. Replaying the same key returns the
    /// original submission rather than creating a second one.
    pub idempotency_key: String,
}

/// Response of `POST /api/v1/submissions` and `GET /api/v1/submissions/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmissionResponse {
    /// Submission identifier.
    pub submission_id: SubmissionId,
    /// Immutable job identifier, stable for the life of the submission.
    pub job_id: String,
    /// Trial slug.
    pub trial: String,
    /// Trial content version being judged against.
    pub trial_version: u32,
    /// Current state, flattened so the wire form is `{"state": "running", ...}`
    /// rather than a nested object.
    #[serde(flatten)]
    pub state: SubmissionState,
    /// Creation time.
    pub created_at: Timestamp,
    /// Whether this response replayed an existing submission for the supplied
    /// idempotency key.
    #[serde(default)]
    pub idempotent_replay: bool,
}

/// A single Server-Sent Event on `GET /api/v1/submissions/{id}/events`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmissionEvent {
    /// Submission the event belongs to.
    pub submission_id: SubmissionId,
    /// New state.
    #[serde(flatten)]
    pub state: SubmissionState,
    /// Server time the transition was observed.
    pub at: Timestamp,
}

/// One entry in the Recent feed or the Archive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEntry {
    /// Event kind.
    pub kind: rustly_domain::EventKind,
    /// Rendered summary.
    pub summary: String,
    /// Relative link, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    /// Subject username, if the event is about a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// When it happened.
    pub occurred_at: Timestamp,
}

/// Response of `GET /api/v1/recent` and `GET /api/v1/archive`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventListResponse {
    /// The events, newest first.
    pub events: Vec<EventEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustly_domain::Verdict;

    #[test]
    fn submission_request_carries_a_cid_not_source() {
        let json = serde_json::json!({
            "trial": "ownership-move-or-borrow",
            "source_cid": "b3:2f1c...",
            "idempotency_key": "01H..."
        });
        let req: CreateSubmissionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.source_cid, "b3:2f1c...");

        // The contract has no field that could carry a program body.
        let value = serde_json::to_value(&req).unwrap();
        let keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), 3);
        assert!(!keys
            .iter()
            .any(|k| k.contains("source") && !k.ends_with("_cid")));
    }

    #[test]
    fn submission_state_flattens_into_the_sse_payload() {
        let event = SubmissionEvent {
            submission_id: SubmissionId::new(),
            state: SubmissionState::Finished {
                verdict: Verdict::Accepted,
            },
            at: Timestamp::now(),
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["state"], "finished");
        assert_eq!(value["verdict"], "AC");
    }

    #[test]
    fn submission_response_flattens_its_state() {
        let response = SubmissionResponse {
            submission_id: SubmissionId::new(),
            job_id: "job-1".into(),
            trial: "ownership-move-or-borrow".into(),
            trial_version: 1,
            state: SubmissionState::Finished {
                verdict: Verdict::Accepted,
            },
            created_at: Timestamp::now(),
            idempotent_replay: false,
        };
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["state"], "finished");
        assert_eq!(value["verdict"], "AC");
        assert_eq!(
            serde_json::from_value::<SubmissionResponse>(value).unwrap(),
            response
        );
    }

    #[test]
    fn error_body_shape_is_fixed() {
        let value = serde_json::to_value(ErrorResponse {
            code: "not_found".into(),
            message: "trial not found".into(),
            request_id: "req-1".into(),
        })
        .unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["code", "message", "request_id"]);
    }

    #[test]
    fn checkpoint_response_reports_a_replay_as_entirely_unchanged() {
        let outcome = rustly_progress::MergeOutcome {
            inserted: 0,
            updated: 0,
            unchanged: 4,
        };
        let response: CheckpointResponse = outcome.into();
        assert_eq!(response.unchanged, 4);
        assert_eq!(response.inserted + response.updated, 0);
    }

    #[test]
    fn trial_query_deserialises_from_partial_parameters() {
        let q: TrialQuery = serde_json::from_str(r#"{"difficulty":"hard"}"#).unwrap();
        assert_eq!(q.difficulty, Some(Difficulty::Hard));
        assert_eq!(q.topic, None);
        assert_eq!(q.status, None);
    }
}
