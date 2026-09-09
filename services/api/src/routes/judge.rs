//! The judge broker: lease, heartbeat, report.
//!
//! Pull-based on purpose. Workers may be behind NAT, may be volunteer capacity,
//! and may disappear mid-job; a push design would need the control plane to
//! track reachability and to hold connections open to untrusted hosts.
//!
//! # The rule this module exists to enforce
//!
//! A worker's [`TrustClass`] comes from its **token**, never from its request
//! body. Hidden tests are dispatched only to `Trusted` workers. A volunteer that
//! claims `"trust_class": "trusted"` in JSON is rejected outright.

use axum::extract::{Path, State};
use axum::Json;
use rustly_auth::{Principal, Scope};
use rustly_common::{Error, Timestamp};
use rustly_domain::submission::JobId;
use rustly_domain::{SubmissionState, TrialMilestone, Verdict};
use rustly_protocol::api::SubmissionEvent;
use rustly_protocol::broker::{
    ExecutionBackend, HeartbeatRequest, JobLease, JobProgress, LeaseRequest, LeaseResponse,
    ResultAck, ResultReport, TrustClass,
};
use rustly_protocol::BROKER_PROTOCOL_VERSION;

use crate::error::ApiError;
use crate::extract::{Caller, RequestId};
use crate::state::AppState;

/// How long a lease is held before the job is re-queued.
pub const LEASE_SECONDS: i64 = 120;

/// Seconds a worker should wait before re-polling an empty queue.
pub const EMPTY_QUEUE_BACKOFF_SECONDS: u32 = 5;

/// Maximum jobs a single lease request may claim.
pub const MAX_LEASE_CAPACITY: u32 = 32;

fn worker(principal: &Principal, request_id: &str) -> Result<(String, TrustClass), ApiError> {
    principal
        .require(Scope::JudgeWork)
        .map_err(|e| ApiError::new(e, request_id.to_owned()))?;
    match principal {
        Principal::Worker {
            id, trust_class, ..
        } => Ok((id.clone(), *trust_class)),
        _ => Err(ApiError::new(
            Error::Forbidden("only judge workers may use the broker".into()),
            request_id.to_owned(),
        )),
    }
}

/// `POST /api/v1/judge/leases`.
pub async fn lease(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Caller(principal): Caller,
    Json(body): Json<LeaseRequest>,
) -> Result<Json<LeaseResponse>, ApiError> {
    let (worker_id, token_trust) = worker(&principal, &request_id)?;

    if body.protocol_version != BROKER_PROTOCOL_VERSION {
        return Err(ApiError::new(
            Error::invalid(
                "protocol_version",
                format!(
                    "broker speaks v{BROKER_PROTOCOL_VERSION}, worker sent v{}",
                    body.protocol_version
                ),
            ),
            request_id,
        ));
    }

    // The body is a claim; the token is the authority. A worker that claims more
    // trust than its credential grants is refused rather than downgraded, so the
    // attempt is visible in logs instead of silently absorbed.
    if body.trust_class > token_trust {
        tracing::warn!(
            worker_id = %worker_id,
            claimed = ?body.trust_class,
            granted = ?token_trust,
            "worker claimed a higher trust class than its credential grants"
        );
        return Err(ApiError::new(
            Error::Forbidden("declared trust class exceeds the credential".into()),
            request_id,
        ));
    }
    if body.worker_id != worker_id {
        return Err(ApiError::new(
            Error::Forbidden("worker_id does not match the credential".into()),
            request_id,
        ));
    }
    if !body.backends.contains(&ExecutionBackend::Wasmtime) {
        // Wasmtime is the only backend qualified for untrusted submissions.
        return Err(ApiError::new(
            Error::invalid("backends", "a worker must offer the wasmtime backend"),
            request_id,
        ));
    }

    let capacity = body.capacity.clamp(1, MAX_LEASE_CAPACITY);
    let leased = state
        .store
        .lease_jobs(&worker_id, token_trust, capacity, LEASE_SECONDS)
        .await
        .map_err(|e| ApiError::new(e, request_id))?;

    let jobs: Vec<JobLease> = leased
        .into_iter()
        .map(|job| {
            let submission_state = SubmissionState::Dispatched {
                worker_id: worker_id.clone(),
            };
            state.publish(SubmissionEvent {
                submission_id: rustly_domain::SubmissionId::from_uuid(job.job_id.as_uuid()),
                state: submission_state,
                at: Timestamp::now(),
            });
            JobLease {
                protocol_version: BROKER_PROTOCOL_VERSION,
                job_id: job.job_id.to_string(),
                source_cid: job.source_cid,
                trial_package_cid: job.trial_package_cid,
                trial_version: job.trial_version,
                environment_id: "rust-1.85-wasm32-wasip1".to_owned(),
                limits: job.limits,
                backend: ExecutionBackend::Wasmtime,
                may_receive_hidden_tests: job.may_receive_hidden_tests,
                lease_expires_at: job.lease_expires_at,
            }
        })
        .collect();

    tracing::info!(worker_id = %worker_id, leased = jobs.len(), "jobs leased");
    Ok(Json(LeaseResponse {
        jobs,
        poll_after_seconds: EMPTY_QUEUE_BACKOFF_SECONDS,
    }))
}

/// `POST /api/v1/judge/jobs/{job_id}/heartbeat`.
pub async fn heartbeat(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Caller(principal): Caller,
    Path(job_id): Path<String>,
    Json(body): Json<HeartbeatRequest>,
) -> Result<axum::http::StatusCode, ApiError> {
    let (worker_id, _) = worker(&principal, &request_id)?;
    if body.worker_id != worker_id {
        return Err(ApiError::new(
            Error::Forbidden("worker_id does not match the credential".into()),
            request_id,
        ));
    }

    let job_id: JobId = job_id
        .parse()
        .map_err(|_| ApiError::new(Error::invalid("job_id", "not a UUID"), request_id.clone()))?;

    let state_update = match body.progress {
        JobProgress::Fetching => SubmissionState::Dispatched {
            worker_id: worker_id.clone(),
        },
        JobProgress::Compiling => SubmissionState::Compiling,
        JobProgress::Running { completed, total } => SubmissionState::Running { completed, total },
    };

    state
        .store
        .update_submission_state(job_id, state_update.clone())
        .await
        .map_err(|e| ApiError::new(e, request_id))?;

    state.publish(SubmissionEvent {
        submission_id: rustly_domain::SubmissionId::from_uuid(job_id.as_uuid()),
        state: state_update,
        at: Timestamp::now(),
    });

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `POST /api/v1/judge/jobs/{job_id}/result`.
///
/// Idempotent. A worker that reports twice, or two workers racing after a lease
/// expiry, cannot double-count a solve or move the rank twice.
pub async fn result(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Caller(principal): Caller,
    Path(job_id): Path<String>,
    Json(body): Json<ResultReport>,
) -> Result<Json<ResultAck>, ApiError> {
    let (worker_id, _) = worker(&principal, &request_id)?;
    if body.worker_id != worker_id {
        return Err(ApiError::new(
            Error::Forbidden("worker_id does not match the credential".into()),
            request_id,
        ));
    }
    if body.protocol_version != BROKER_PROTOCOL_VERSION {
        return Err(ApiError::new(
            Error::invalid("protocol_version", "unsupported broker protocol version"),
            request_id,
        ));
    }

    let job_id: JobId = job_id
        .parse()
        .map_err(|_| ApiError::new(Error::invalid("job_id", "not a UUID"), request_id.clone()))?;

    let outcome = state
        .store
        .record_result(
            job_id,
            &worker_id,
            &body.trial_package_cid,
            body.verdict,
            &body.result_manifest_hash,
        )
        .await
        .map_err(|e| ApiError::new(e, request_id.clone()))?;

    tracing::info!(
        job_id = %job_id,
        worker_id = %worker_id,
        verdict = body.verdict.code(),
        accepted = outcome.accepted,
        first_solve = outcome.first_solve,
        used_cached_artifact = body.used_cached_artifact,
        compile_ms = body.compile_ms,
        execution_ms = body.execution_ms,
        "judge result recorded"
    );

    if outcome.accepted {
        state.publish(SubmissionEvent {
            submission_id: rustly_domain::SubmissionId::from_uuid(job_id.as_uuid()),
            state: SubmissionState::Finished {
                verdict: outcome.recorded_verdict,
            },
            at: Timestamp::now(),
        });
    }

    // A milestone event is emitted once per threshold crossing, never per solve.
    if outcome.first_solve && outcome.recorded_verdict == Verdict::Accepted {
        let solved = outcome.user.trials_solved;
        if TrialMilestone::highest_for(solved)
            != TrialMilestone::highest_for(solved.saturating_sub(1))
        {
            if let Some(milestone) = TrialMilestone::highest_for(solved) {
                let event =
                    rustly_community::feed::milestone_event(&outcome.user.username, milestone);
                if let Err(error) = state.store.append_event(&event).await {
                    // A feed write must never fail a judged submission.
                    tracing::warn!(%error, "failed to append milestone event");
                }
            }
        }
    }

    Ok(Json(ResultAck {
        accepted: outcome.accepted,
        recorded_verdict: outcome.recorded_verdict,
    }))
}
