//! Minimal guest authentication for the first hosted Trial.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use rustly_auth::Scope;
use rustly_common::{Error, Timestamp};
use rustly_domain::Username;
use rustly_protocol::api::GuestSessionResponse;

use crate::error::ApiError;
use crate::extract::RequestId;
use crate::rate_limit::client_key;
use crate::state::AppState;

const SESSION_SECONDS: i64 = 24 * 60 * 60;

/// `POST /api/v1/auth/guest` creates a durable guest identity and bearer token.
pub async fn guest(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    headers: HeaderMap,
) -> Result<Json<GuestSessionResponse>, ApiError> {
    state
        .rate_limits
        .check(
            "guest_session",
            &client_key(&headers),
            5,
            std::time::Duration::from_secs(60 * 60),
        )
        .map_err(|error| ApiError::new(error, request_id.clone()))?;
    for _ in 0..4 {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let username = Username::parse(&format!("guest-{}", &suffix[..12]))
            .expect("generated guest username is valid");
        match state.store.create_user(&username).await {
            Ok(user) => {
                let now = Timestamp::now();
                let access_token =
                    state
                        .tokens
                        .issue_user(user.id, &Scope::USER_DEFAULT, now, SESSION_SECONDS);
                return Ok(Json(GuestSessionResponse {
                    access_token,
                    expires_at: now.unix_seconds() + SESSION_SECONDS,
                    username: user.username.to_string(),
                }));
            }
            Err(Error::Conflict(_)) => continue,
            Err(error) => return Err(ApiError::new(error, request_id)),
        }
    }
    Err(ApiError::new(
        Error::Internal("could not allocate guest identity".into()),
        request_id,
    ))
}
