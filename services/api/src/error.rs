//! HTTP mapping for [`rustly_common::Error`].
//!
//! There is exactly one place where a domain error becomes a status code. That
//! matters: a new error variant cannot accidentally become a 500, and an
//! infrastructure failure cannot leak a connection string to a caller.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rustly_common::Error;
use rustly_protocol::api::ErrorResponse;

/// An error carrying the request id it happened under.
#[derive(Debug)]
pub struct ApiError {
    /// The underlying failure.
    pub error: Error,
    /// Correlation id echoed to the caller.
    pub request_id: String,
}

impl ApiError {
    /// Attach a request id to a domain error.
    pub fn new(error: Error, request_id: impl Into<String>) -> Self {
        Self {
            error,
            request_id: request_id.into(),
        }
    }

    /// The HTTP status for this error.
    pub fn status(&self) -> StatusCode {
        match &self.error {
            Error::NotFound { .. } => StatusCode::NOT_FOUND,
            Error::Invalid { .. } => StatusCode::BAD_REQUEST,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::Unauthenticated(_) => StatusCode::UNAUTHORIZED,
            Error::Forbidden(_) => StatusCode::FORBIDDEN,
            Error::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Error::Dependency { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();

        // Client-safe messages go out verbatim. Everything else is logged in
        // full and summarised, so a stack of internal detail never reaches a
        // caller who might be probing.
        let message = if self.error.is_client_safe() {
            self.error.to_string()
        } else {
            tracing::error!(
                request_id = %self.request_id,
                error = %self.error,
                "request failed"
            );
            match self.error {
                Error::Dependency { dependency, .. } => {
                    format!("a dependency ({dependency}) is unavailable; please retry")
                }
                _ => "internal error".to_owned(),
            }
        };

        let body = ErrorResponse {
            code: self.error_code(status),
            message,
            request_id: self.request_id,
        };
        let retry_after = match self.error {
            Error::RateLimited {
                retry_after_seconds,
            } => Some(retry_after_seconds),
            _ => None,
        };
        let mut response = (status, Json(body)).into_response();
        if let Some(seconds) = retry_after {
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        response
    }
}

impl ApiError {
    fn error_code(&self, status: StatusCode) -> String {
        let _ = status;
        self.error.code().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping_is_exhaustive_and_stable() {
        let cases = [
            (Error::not_found("trial", "x"), StatusCode::NOT_FOUND),
            (Error::invalid("f", "r"), StatusCode::BAD_REQUEST),
            (Error::Conflict("dup".into()), StatusCode::CONFLICT),
            (
                Error::Unauthenticated("no".into()),
                StatusCode::UNAUTHORIZED,
            ),
            (Error::Forbidden("no".into()), StatusCode::FORBIDDEN),
            (
                Error::RateLimited {
                    retry_after_seconds: 1,
                },
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (
                Error::dependency("postgres", std::io::Error::other("down")),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                Error::Internal("bug".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(ApiError::new(error, "req").status(), expected);
        }
    }

    #[tokio::test]
    async fn internal_detail_never_reaches_the_caller() {
        let io = std::io::Error::other("password=hunter2 host=10.0.0.4");
        let response = ApiError::new(Error::dependency("postgres", io), "req-1").into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("hunter2"), "leaked a secret: {text}");
        assert!(
            !text.contains("10.0.0.4"),
            "leaked an internal host: {text}"
        );
        assert!(text.contains("dependency_unavailable"));
        assert!(text.contains("req-1"));
    }

    #[tokio::test]
    async fn client_safe_messages_are_returned_verbatim() {
        let response =
            ApiError::new(Error::not_found("trial", "ownership"), "req-2").into_response();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("trial not found: ownership"));
        assert!(text.contains("not_found"));
    }
}
