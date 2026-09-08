//! Liveness, readiness, and version.

use axum::extract::State;
use axum::Json;
use rustly_protocol::api::{DependencyCheck, HealthResponse, VersionResponse};
use rustly_protocol::{API_VERSION, BROKER_PROTOCOL_VERSION};

use crate::state::AppState;

/// `GET /health` - liveness. Deliberately touches no dependency, so a database
/// outage does not cause an orchestrator to kill an otherwise healthy process.
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        checks: vec![],
    })
}

/// `GET /ready` - readiness. Pings the metadata store.
pub async fn ready(
    State(state): State<AppState>,
) -> (axum::http::StatusCode, Json<HealthResponse>) {
    let (healthy, detail) = match state.store.ping().await {
        Ok(()) => (true, None),
        Err(error) => {
            tracing::warn!(%error, "readiness check failed");
            (false, Some("unreachable".to_owned()))
        }
    };

    let status = if healthy {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(HealthResponse {
            status: if healthy { "ok" } else { "degraded" },
            checks: vec![DependencyCheck {
                name: "metadata_store".into(),
                healthy,
                detail,
            }],
        }),
    )
}

/// `GET /api/version`.
pub async fn version(State(state): State<AppState>) -> Json<VersionResponse> {
    Json(VersionResponse {
        api_version: API_VERSION,
        broker_protocol_version: BROKER_PROTOCOL_VERSION,
        build: state.build.clone(),
        ranking_model: state.ranking.id().to_owned(),
        // Surfaced so the UI can label the rank number honestly.
        ranking_provisional: state.ranking.is_provisional(),
    })
}
