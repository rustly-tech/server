//! Normalizes framework-generated failures and request identifiers.

use axum::body::Body;
use axum::http::{header, HeaderValue, Request, Response, StatusCode};
use axum::middleware::Next;
use rustly_protocol::api::ErrorResponse;

use crate::extract::{RequestId, REQUEST_ID_HEADER};

/// Ensure every response carries a request id and every HTTP failure is JSON.
pub async fn normalize(mut request: Request<Body>, next: Next) -> Response<Body> {
    let raw = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok());
    let request_id = RequestId::from_header(raw);
    if let Ok(value) = HeaderValue::from_str(&request_id.0) {
        request.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    request.extensions_mut().insert(request_id.clone());

    let response = next.run(request).await;
    let status = response.status();
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    let mut response = if status.is_success() || is_json {
        response
    } else {
        let (parts, _) = response.into_parts();
        let (code, message) = framework_error(status);
        let bytes = serde_json::to_vec(&ErrorResponse {
            code: code.into(),
            message: message.into(),
            request_id: request_id.0.clone(),
        })
        .expect("error envelope is serializable");
        let mut normalized = Response::from_parts(parts, Body::from(bytes));
        normalized.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        normalized
    };
    if let Ok(value) = HeaderValue::from_str(&request_id.0) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

fn framework_error(status: StatusCode) -> (&'static str, &'static str) {
    match status {
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            ("invalid_request", "request body or parameters are invalid")
        }
        StatusCode::NOT_FOUND => ("not_found", "route not found"),
        StatusCode::METHOD_NOT_ALLOWED => ("method_not_allowed", "method not allowed"),
        StatusCode::REQUEST_TIMEOUT => ("request_timeout", "request timed out"),
        StatusCode::PAYLOAD_TOO_LARGE => ("payload_too_large", "request body is too large"),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => (
            "unsupported_media_type",
            "Content-Type must be application/json",
        ),
        _ => ("http_error", "request failed"),
    }
}
