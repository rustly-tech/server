//! Direct source-upload capability issuance.

use axum::extract::State;
use axum::Json;
use rustly_auth::Scope;
use rustly_common::{Error, Timestamp};
use rustly_protocol::api::{UploadGrantRequest, UploadGrantResponse};
use rustly_upload_protocol::UploadGrant;

use crate::error::ApiError;
use crate::extract::{Caller, RequestId};
use crate::state::AppState;

const MAX_SOURCE_BYTES: u64 = 256 * 1024;
const GRANT_SECONDS: i64 = 5 * 60;

fn valid_cid(cid: &str) -> bool {
    cid.strip_prefix("b3:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

/// `POST /api/v1/uploads/source`.
pub async fn grant(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Caller(principal): Caller,
    Json(body): Json<UploadGrantRequest>,
) -> Result<Json<UploadGrantResponse>, ApiError> {
    principal
        .require(Scope::Submit)
        .map_err(|e| ApiError::new(e, request_id.clone()))?;
    let user_id = principal
        .user_id()
        .ok_or_else(|| ApiError::new(Error::Forbidden("not a user".into()), request_id.clone()))?;
    state
        .rate_limits
        .check(
            "source_upload_grant",
            &user_id.to_string(),
            20,
            std::time::Duration::from_secs(60),
        )
        .map_err(|error| ApiError::new(error, request_id.clone()))?;
    if !valid_cid(&body.cid) {
        return Err(ApiError::new(
            Error::invalid("cid", "must be a canonical BLAKE3 CID"),
            request_id,
        ));
    }
    if body.size == 0 || body.size > MAX_SOURCE_BYTES {
        return Err(ApiError::new(
            Error::invalid("size", format!("must be 1-{MAX_SOURCE_BYTES} bytes")),
            request_id,
        ));
    }
    let expires_at = Timestamp::now().unix_seconds() + GRANT_SECONDS;
    let grant = UploadGrant {
        user_id: user_id.to_string(),
        cid: body.cid.clone(),
        size: body.size,
        expires_at,
        nonce: uuid::Uuid::new_v4().to_string(),
    };
    Ok(Json(UploadGrantResponse {
        upload_url: format!("{}/api/v1/uploads/{}", state.artifact_gateway_url, body.cid),
        grant: state.upload_tokens.sign_grant(&grant),
        expires_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::valid_cid;

    #[test]
    fn only_canonical_blake3_cids_are_accepted() {
        assert!(valid_cid(&format!("b3:{}", "a".repeat(64))));
        assert!(!valid_cid("b3:abcd"));
        assert!(!valid_cid(&format!("sha256:{}", "a".repeat(64))));
    }
}
