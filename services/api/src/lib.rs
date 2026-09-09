//! The Rustly control-plane HTTP API.
//!
//! # Shape
//!
//! Small, trusted, and boring. It owns identity, permissions, ranking, Git refs,
//! hidden tests, entitlement, moderation state, and accepted verdicts - and
//! nothing else. It does not serve lessons, does not serve Trial statements,
//! does not carry blobs, and above all **does not execute user code**: the judge
//! is a separate process behind a pull-based broker.
//!
//! # Endpoints
//!
//! | Method | Path | Purpose |
//! | --- | --- | --- |
//! | GET | `/health` | Liveness. Never touches a dependency. |
//! | GET | `/ready` | Readiness. Pings the metadata store. |
//! | GET | `/api/version` | API and broker protocol versions, ranking model id. |
//! | GET | `/api/v1/users/{username}` | Minimal public profile. |
//! | GET | `/api/v1/me` | The caller's own account. |
//! | POST | `/api/v1/auth/guest` | Create a guest identity and session. |
//! | POST | `/api/v1/uploads/source` | Create a direct source-upload grant. |
//! | GET | `/api/v1/trials` | Listed Trials, filterable. |
//! | GET | `/api/v1/trials/{slug}` | One Trial's metadata. |
//! | POST | `/api/v1/trials/{slug}/reveal` | Reveal solutions, forfeiting rank credit. |
//! | GET | `/api/v1/progress` | Merged progress. |
//! | POST | `/api/v1/progress/checkpoints` | Merge a compact checkpoint batch. |
//! | POST | `/api/v1/submissions` | Create a submission from a source CID. |
//! | GET | `/api/v1/submissions/{id}` | Submission status. |
//! | GET | `/api/v1/submissions/{id}/events` | SSE status stream. |
//! | POST | `/api/v1/judge/leases` | Worker leases jobs. |
//! | POST | `/api/v1/judge/jobs/{job_id}/heartbeat` | Worker progress. |
//! | POST | `/api/v1/judge/jobs/{job_id}/result` | Worker reports a verdict. |
//! | GET | `/api/v1/recent` | 24-hour meaningful-event window. |
//! | GET | `/api/v1/archive` | Full meaningful-event history. |
//! | GET | `/api/v1/clans/{tag}` | Clan identity and members. |

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod config;
pub mod error;
pub mod extract;
pub mod http_contract;
pub mod rate_limit;
pub mod routes;
pub mod seed;
pub mod state;

pub use state::AppState;

/// Build the router for a given application state.
pub fn app(state: AppState) -> axum::Router {
    routes::router(state)
}
