//! Local-first progress synchronisation.
//!
//! The client owns progress. This endpoint accepts compact, bounded checkpoint
//! batches and merges them idempotently. It never receives keystrokes, editor
//! heartbeats, lesson views, or individual quiz clicks (invariant C).

use axum::extract::State;
use axum::Json;
use rustly_auth::Scope;
use rustly_protocol::api::{CheckpointRequest, CheckpointResponse, ProgressResponse};

use crate::error::ApiError;
use crate::extract::{Caller, RequestId};
use crate::state::AppState;

/// `GET /api/v1/progress`.
pub async fn read(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Caller(principal): Caller,
) -> Result<Json<ProgressResponse>, ApiError> {
    principal
        .require(Scope::ReadSelf)
        .map_err(|e| ApiError::new(e, request_id.clone()))?;
    let user_id = principal.user_id().ok_or_else(|| {
        ApiError::new(
            rustly_common::Error::Forbidden("not a user".into()),
            request_id.clone(),
        )
    })?;

    let state_snapshot = state
        .store
        .progress(user_id)
        .await
        .map_err(|e| ApiError::new(e, request_id))?;

    let completed = state_snapshot.completed_count();
    Ok(Json(ProgressResponse {
        entries: state_snapshot
            .entries
            .into_iter()
            .map(|(key, entry)| (key, entry.completion))
            .collect(),
        completed,
    }))
}

/// `POST /api/v1/progress/checkpoints`.
pub async fn checkpoint(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Caller(principal): Caller,
    Json(body): Json<CheckpointRequest>,
) -> Result<Json<CheckpointResponse>, ApiError> {
    principal
        .require(Scope::WriteProgress)
        .map_err(|e| ApiError::new(e, request_id.clone()))?;
    let user_id = principal.user_id().ok_or_else(|| {
        ApiError::new(
            rustly_common::Error::Forbidden("not a user".into()),
            request_id.clone(),
        )
    })?;

    let outcome = state
        .store
        .merge_checkpoint(user_id, &body.checkpoint)
        .await
        .map_err(|e| ApiError::new(e, request_id))?;

    Ok(Json(outcome.into()))
}
