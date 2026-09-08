//! Request extractors: correlation id and authenticated principal.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use rustly_auth::{bearer, Principal};
use rustly_common::{Error, Timestamp};

use crate::error::ApiError;
use crate::state::AppState;

/// Header carrying the correlation id, in and out.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// The correlation id for this request.
///
/// A caller-supplied `x-request-id` is honoured so a trace can span the browser,
/// the API, and the judge - but only if it looks like an id. Echoing arbitrary
/// caller input into logs and response bodies is a log-injection vector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId(pub String);

impl RequestId {
    /// Maximum accepted length of a caller-supplied id.
    pub const MAX_LEN: usize = 64;

    /// Sanitise a caller-supplied value, or mint a fresh one.
    pub fn from_header(raw: Option<&str>) -> Self {
        match raw {
            Some(value)
                if !value.is_empty()
                    && value.len() <= Self::MAX_LEN
                    && value
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) =>
            {
                Self(value.to_owned())
            }
            _ => Self(uuid::Uuid::now_v7().to_string()),
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for RequestId {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let raw = parts
            .headers
            .get(REQUEST_ID_HEADER)
            .and_then(|v| v.to_str().ok());
        let id = Self::from_header(raw);
        parts.extensions.insert(id.clone());
        Ok(id)
    }
}

/// The authenticated caller, or [`Principal::Anonymous`].
///
/// A malformed or expired token is an error rather than a silent downgrade to
/// anonymous: a user whose session expired should be told, not quietly served a
/// logged-out page.
#[derive(Debug, Clone)]
pub struct Caller(pub Principal);

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let request_id = RequestId::from_request_parts(parts, state)
            .await
            .unwrap_or_else(|e| match e {});

        let Some(header) = parts.headers.get(axum::http::header::AUTHORIZATION) else {
            return Ok(Self(Principal::Anonymous));
        };
        let value = header.to_str().map_err(|_| {
            ApiError::new(
                Error::Unauthenticated("malformed header".into()),
                request_id.0.clone(),
            )
        })?;
        let Some(token) = bearer(value) else {
            return Err(ApiError::new(
                Error::Unauthenticated("expected a Bearer token".into()),
                request_id.0,
            ));
        };

        let principal = state
            .tokens
            .verify(token, Timestamp::now())
            .map_err(|e| ApiError::new(e, request_id.0))?;
        Ok(Self(principal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_well_formed_caller_id_is_honoured() {
        assert_eq!(RequestId::from_header(Some("abc-123_x.y")).0, "abc-123_x.y");
    }

    #[test]
    fn hostile_caller_ids_are_replaced_not_echoed() {
        for hostile in [
            "with space",
            "new\nline",
            "quote\"",
            "<script>",
            &"a".repeat(RequestId::MAX_LEN + 1),
            "",
        ] {
            let id = RequestId::from_header(Some(hostile)).0;
            assert_ne!(id, hostile, "must not echo {hostile:?}");
            assert!(
                uuid::Uuid::parse_str(&id).is_ok(),
                "replacement must be a UUID"
            );
        }
    }

    #[test]
    fn a_missing_header_mints_an_id() {
        assert!(uuid::Uuid::parse_str(&RequestId::from_header(None).0).is_ok());
    }
}
