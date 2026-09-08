//! Profile endpoints.

use axum::extract::{Path, State};
use axum::Json;
use rustly_auth::Scope;
use rustly_protocol::api::{MeResponse, ProfileResponse};

use crate::error::ApiError;
use crate::extract::{Caller, RequestId};
use crate::state::AppState;

/// `GET /api/v1/users/{username}` - the minimal public profile.
///
/// Seven fields, no country ranking, no performance history, no heatmap. The
/// shape is enforced by `rustly_domain::PublicProfile`.
pub async fn profile(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Path(username): Path<String>,
) -> Result<Json<ProfileResponse>, ApiError> {
    let user = state
        .store
        .user_by_username(&username)
        .await
        .map_err(|e| ApiError::new(e, request_id.clone()))?;
    let global_rank = state
        .store
        .global_rank(user.id)
        .await
        .map_err(|e| ApiError::new(e, request_id))?;

    Ok(Json(ProfileResponse {
        profile: user.public_profile(global_rank),
    }))
}

/// `GET /api/v1/me` - the caller's own account.
pub async fn me(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Caller(principal): Caller,
) -> Result<Json<MeResponse>, ApiError> {
    principal
        .require(Scope::ReadSelf)
        .map_err(|e| ApiError::new(e, request_id.clone()))?;
    let id = principal.user_id().ok_or_else(|| {
        ApiError::new(
            rustly_common::Error::Forbidden("not a user".into()),
            request_id.clone(),
        )
    })?;

    let user = state
        .store
        .user_by_id(id)
        .await
        .map_err(|e| ApiError::new(e, request_id))?;
    Ok(Json(MeResponse {
        username: user.username,
        rank: user.rank_score,
        level: user.level,
        trials_solved: user.trials_solved,
        supporter: user.supporter,
    }))
}
