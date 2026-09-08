//! Trial metadata endpoints.
//!
//! These serve *metadata* only. Statements, starter code, and public tests are
//! static content fetched by CID, which is what keeps ordinary Trial reads off
//! the API entirely (invariant A).

use axum::extract::{Path, Query, State};
use axum::Json;
use rustly_auth::Scope;
use rustly_domain::{TrialSlug, TrialStatus};
use rustly_protocol::api::{TrialListResponse, TrialQuery, TrialSummary};
use rustly_storage::TrialFilter;

use crate::error::ApiError;
use crate::extract::{Caller, RequestId};
use crate::state::AppState;

/// Maximum Trials returned in one listing.
pub const LIST_LIMIT: usize = 200;

/// `GET /api/v1/trials`.
pub async fn list(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Caller(principal): Caller,
    Query(query): Query<TrialQuery>,
) -> Result<Json<TrialListResponse>, ApiError> {
    let filter = TrialFilter {
        difficulty: query.difficulty,
        topic: query.topic,
        limit: LIST_LIMIT,
    };
    let trials = state
        .store
        .list_trials(&filter)
        .await
        .map_err(|e| ApiError::new(e, request_id.clone()))?;

    let user_id = principal.user_id();
    let mut summaries = Vec::with_capacity(trials.len());
    for trial in trials {
        let status = match user_id {
            None => TrialStatus::Unsolved,
            Some(id) => {
                state
                    .store
                    .trial_user_state(id, trial.id)
                    .await
                    .map_err(|e| ApiError::new(e, request_id.clone()))?
                    .status
            }
        };
        if query.status.is_some_and(|wanted| wanted != status) {
            continue;
        }
        summaries.push(summary(trial, status));
    }

    Ok(Json(TrialListResponse { trials: summaries }))
}

/// `GET /api/v1/trials/{slug}`.
pub async fn detail(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Caller(principal): Caller,
    Path(slug): Path<String>,
) -> Result<Json<TrialSummary>, ApiError> {
    let slug = TrialSlug::parse(&slug).map_err(|e| ApiError::new(e, request_id.clone()))?;
    let trial = state
        .store
        .trial_by_slug(&slug)
        .await
        .map_err(|e| ApiError::new(e, request_id.clone()))?;

    // A draft Trial is indistinguishable from a missing one, so an unpublished
    // slug cannot be probed for.
    if !trial.is_listed() {
        return Err(ApiError::new(
            rustly_common::Error::not_found("trial", slug),
            request_id,
        ));
    }

    let status = match principal.user_id() {
        None => TrialStatus::Unsolved,
        Some(id) => {
            state
                .store
                .trial_user_state(id, trial.id)
                .await
                .map_err(|e| ApiError::new(e, request_id))?
                .status
        }
    };

    Ok(Json(summary(trial, status)))
}

/// `POST /api/v1/trials/{slug}/reveal`.
///
/// Revealing published solutions is explicitly allowed - it is often the fastest
/// way to learn. It removes first-solve ranking credit for that Trial and
/// nothing else: the solve still counts toward the milestone badge, and the
/// experience is still awarded.
pub async fn reveal(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Caller(principal): Caller,
    Path(slug): Path<String>,
) -> Result<Json<TrialSummary>, ApiError> {
    principal
        .require(Scope::Submit)
        .map_err(|e| ApiError::new(e, request_id.clone()))?;
    let user_id = principal.user_id().ok_or_else(|| {
        ApiError::new(
            rustly_common::Error::Forbidden("not a user".into()),
            request_id.clone(),
        )
    })?;

    let slug = TrialSlug::parse(&slug).map_err(|e| ApiError::new(e, request_id.clone()))?;
    let trial = state
        .store
        .trial_by_slug(&slug)
        .await
        .map_err(|e| ApiError::new(e, request_id.clone()))?;

    state
        .store
        .reveal_solutions(user_id, trial.id)
        .await
        .map_err(|e| ApiError::new(e, request_id.clone()))?;

    let status = state
        .store
        .trial_user_state(user_id, trial.id)
        .await
        .map_err(|e| ApiError::new(e, request_id))?
        .status;

    Ok(Json(summary(trial, status)))
}

fn summary(trial: rustly_domain::Trial, status: TrialStatus) -> TrialSummary {
    TrialSummary {
        slug: trial.slug,
        title: trial.title,
        difficulty: trial.difficulty,
        topics: trial.topics,
        lifecycle: trial.lifecycle,
        version: trial.version,
        content_cid: trial.content_cid,
        status,
    }
}
