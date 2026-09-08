//! Submission creation, status, and the SSE status stream.
//!
//! The API **never** executes a submission. It records metadata, mints an
//! immutable job id, and enqueues it. A judge worker leases it later
//! (invariant G).

use std::convert::Infallible;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_util::stream::{self, Stream};
use futures_util::StreamExt as _;
use rustly_auth::Scope;
use rustly_common::{Error, Timestamp};
use rustly_domain::{SubmissionId, TrialSlug};
use rustly_protocol::api::{CreateSubmissionRequest, SubmissionEvent, SubmissionResponse};
use rustly_storage::NewSubmission;
use tokio_stream::wrappers::BroadcastStream;

use crate::error::ApiError;
use crate::extract::{Caller, RequestId};
use crate::state::AppState;

/// Maximum accepted CID length.
const MAX_CID_LEN: usize = 128;
/// Maximum accepted idempotency key length.
const MAX_IDEMPOTENCY_KEY_LEN: usize = 128;

fn validate_cid(cid: &str) -> Result<(), Error> {
    if cid.is_empty() || cid.len() > MAX_CID_LEN {
        return Err(Error::invalid("source_cid", "must be 1-128 characters"));
    }
    if !cid
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_'))
    {
        return Err(Error::invalid(
            "source_cid",
            "may contain only alphanumerics, ':', '-' and '_'",
        ));
    }
    Ok(())
}

/// `POST /api/v1/submissions`.
pub async fn create(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Caller(principal): Caller,
    Json(body): Json<CreateSubmissionRequest>,
) -> Result<(axum::http::StatusCode, Json<SubmissionResponse>), ApiError> {
    principal
        .require(Scope::Submit)
        .map_err(|e| ApiError::new(e, request_id.clone()))?;
    let user_id = principal
        .user_id()
        .ok_or_else(|| ApiError::new(Error::Forbidden("not a user".into()), request_id.clone()))?;

    validate_cid(&body.source_cid).map_err(|e| ApiError::new(e, request_id.clone()))?;
    if body.idempotency_key.is_empty() || body.idempotency_key.len() > MAX_IDEMPOTENCY_KEY_LEN {
        return Err(ApiError::new(
            Error::invalid("idempotency_key", "must be 1-128 characters"),
            request_id,
        ));
    }

    let slug = TrialSlug::parse(&body.trial).map_err(|e| ApiError::new(e, request_id.clone()))?;
    let trial = state
        .store
        .trial_by_slug(&slug)
        .await
        .map_err(|e| ApiError::new(e, request_id.clone()))?;
    if !trial.is_listed() {
        return Err(ApiError::new(Error::not_found("trial", slug), request_id));
    }

    let (submission, replayed) = state
        .store
        .create_submission(NewSubmission {
            user_id,
            trial_id: trial.id,
            trial_version: trial.version,
            source_cid: body.source_cid,
            idempotency_key: body.idempotency_key,
        })
        .await
        .map_err(|e| ApiError::new(e, request_id))?;

    if !replayed {
        tracing::info!(
            submission_id = %submission.id,
            job_id = %submission.job_id,
            trial_id = %trial.id,
            trial_version = trial.version,
            user_ref = %principal.log_ref(),
            "submission queued"
        );
        state.publish(SubmissionEvent {
            submission_id: submission.id,
            state: submission.state.clone(),
            at: Timestamp::now(),
        });
    }

    let status = if replayed {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::ACCEPTED
    };

    Ok((
        status,
        Json(response(submission, trial.slug.to_string(), replayed)),
    ))
}

/// `GET /api/v1/submissions/{id}`.
pub async fn detail(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Caller(principal): Caller,
    Path(id): Path<String>,
) -> Result<Json<SubmissionResponse>, ApiError> {
    let submission = load_own(&state, &principal, &id, &request_id).await?;
    let trial = state
        .store
        .list_trials(&rustly_storage::TrialFilter::default())
        .await
        .map_err(|e| ApiError::new(e, request_id.clone()))?
        .into_iter()
        .find(|t| t.id == submission.trial_id);
    let slug = trial.map(|t| t.slug.to_string()).unwrap_or_default();
    Ok(Json(response(submission, slug, false)))
}

/// `GET /api/v1/submissions/{id}/events` - Server-Sent Events.
///
/// SSE rather than a WebSocket: this is a short-lived, server-to-client,
/// text-only stream. A WebSocket would add a second transport, a second set of
/// proxy problems, and no capability we need. WebSockets are reserved for
/// genuinely bidirectional realtime features.
pub async fn stream(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Caller(principal): Caller,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let submission = load_own(&state, &principal, &id, &request_id).await?;
    let submission_id = submission.id;

    let initial = SubmissionEvent {
        submission_id,
        state: submission.state.clone(),
        at: Timestamp::now(),
    };

    // Subscribe *before* emitting the current state, so a transition happening
    // between the read and the subscribe cannot be missed.
    let live = BroadcastStream::new(state.events.subscribe()).filter_map(move |item| async move {
        match item {
            Ok(event) if event.submission_id == submission_id => Some(event),
            // A lagged subscriber has missed transitions. Dropping is correct:
            // the client re-reads authoritative state with a plain GET rather
            // than acting on a partial stream.
            _ => None,
        }
    });

    // Close the stream after the terminal transition has been delivered.
    let mut finished = false;
    let stream = stream::once(async move { initial })
        .chain(live)
        .take_while(move |event: &SubmissionEvent| {
            let stop = finished;
            if event.state.is_terminal() {
                finished = true;
            }
            async move { !stop }
        })
        .map(|event| Ok(sse_event(&event)));

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

fn sse_event(event: &SubmissionEvent) -> Event {
    Event::default()
        .event("submission")
        .json_data(event)
        .unwrap_or_else(|_| Event::default().event("error").data("serialisation failed"))
}

async fn load_own(
    state: &AppState,
    principal: &rustly_auth::Principal,
    id: &str,
    request_id: &str,
) -> Result<rustly_domain::Submission, ApiError> {
    principal
        .require(Scope::ReadSelf)
        .map_err(|e| ApiError::new(e, request_id.to_owned()))?;
    let user_id = principal.user_id().ok_or_else(|| {
        ApiError::new(Error::Forbidden("not a user".into()), request_id.to_owned())
    })?;

    let submission_id: SubmissionId = id.parse().map_err(|_| {
        ApiError::new(
            Error::invalid("submission_id", "not a UUID"),
            request_id.to_owned(),
        )
    })?;
    let submission = state
        .store
        .submission(submission_id)
        .await
        .map_err(|e| ApiError::new(e, request_id.to_owned()))?;

    // Someone else's submission is reported as missing rather than forbidden, so
    // submission ids cannot be probed for existence.
    if submission.user_id != user_id {
        return Err(ApiError::new(
            Error::not_found("submission", submission_id),
            request_id.to_owned(),
        ));
    }
    Ok(submission)
}

fn response(
    submission: rustly_domain::Submission,
    trial: String,
    idempotent_replay: bool,
) -> SubmissionResponse {
    SubmissionResponse {
        submission_id: submission.id,
        job_id: submission.job_id.to_string(),
        trial,
        trial_version: submission.trial_version,
        state: submission.state,
        created_at: submission.created_at,
        idempotent_replay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cid_validation_rejects_anything_that_could_carry_source() {
        assert!(validate_cid("b3:2f1c9a").is_ok());
        assert!(validate_cid("").is_err());
        assert!(validate_cid(&"a".repeat(MAX_CID_LEN + 1)).is_err());
        assert!(validate_cid("fn main() {}").is_err());
        assert!(validate_cid("has space").is_err());
        assert!(validate_cid("new\nline").is_err());
    }
}
