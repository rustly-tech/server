//! HTTP routing.

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub mod auth;
pub mod health;
pub mod judge;
pub mod progress;
pub mod social;
pub mod submissions;
pub mod trials;
pub mod uploads;
pub mod users;

/// Maximum accepted request body.
///
/// The API carries metadata and CIDs, never program source or test data, so a
/// 64 KiB ceiling is generous. This is the mechanical enforcement of
/// "large immutable bytes do not travel through the control plane".
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// Wall-clock ceiling for a single request.
pub const REQUEST_TIMEOUT_SECONDS: u64 = 15;

/// Build the router.
pub fn router(state: AppState) -> Router {
    let v1 = Router::new()
        .route("/auth/guest", post(auth::guest))
        .route("/uploads/source", post(uploads::grant))
        .route("/users/{username}", get(users::profile))
        .route("/me", get(users::me))
        .route("/trials", get(trials::list))
        .route("/trials/{slug}", get(trials::detail))
        .route("/trials/{slug}/reveal", post(trials::reveal))
        .route("/progress", get(progress::read))
        .route("/progress/checkpoints", post(progress::checkpoint))
        .route("/submissions", post(submissions::create))
        .route("/submissions/{id}", get(submissions::detail))
        .route("/judge/leases", post(judge::lease))
        .route("/judge/jobs/{job_id}/heartbeat", post(judge::heartbeat))
        .route("/judge/jobs/{job_id}/result", post(judge::result))
        .route("/recent", get(social::recent))
        .route("/archive", get(social::archive))
        .route("/clans/{tag}", get(social::clan))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(REQUEST_TIMEOUT_SECONDS),
        ))
        // The SSE stream is deliberately outside the request timeout. It ends
        // after a terminal verdict and is bounded by the broadcast buffer.
        .route("/submissions/{id}/events", get(submissions::stream));

    Router::new()
        .route("/health", get(health::health))
        .route("/ready", get(health::ready))
        .route("/api/version", get(health::version))
        .nest("/api/v1", v1)
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(
            CorsLayer::new()
                .allow_origin([
                    "https://rustly.tech".parse().expect("static origin"),
                    "https://www.rustly.tech".parse().expect("static origin"),
                    "https://rustly-tech.github.io"
                        .parse()
                        .expect("static origin"),
                    "http://localhost:4321".parse().expect("static origin"),
                ])
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::OPTIONS,
                ])
                .allow_headers([
                    axum::http::header::AUTHORIZATION,
                    axum::http::header::CONTENT_TYPE,
                    axum::http::HeaderName::from_static("x-request-id"),
                ])
                .expose_headers([
                    axum::http::HeaderName::from_static("x-request-id"),
                    axum::http::header::RETRY_AFTER,
                ]),
        )
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(crate::http_contract::normalize))
        .with_state(state)
}
