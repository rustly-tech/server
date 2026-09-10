//! End-to-end HTTP test of the Ownership vertical slice.
//!
//! Drives the real router with the real handlers over the in-memory metadata
//! store, which the `rustly-storage` conformance suite proves behaves
//! identically to PostgreSQL. Nothing here is mocked at the HTTP layer.
//!
//! The path exercised is the whole product loop that has to work for the slice
//! to be real: read a Trial, sync local-first progress, submit a source CID,
//! have a worker lease and judge it, and see rank, level, status, and the
//! Recent feed change as a result.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt as _;
use rustly_api::{seed, AppState};
use rustly_auth::{Scope, TokenIssuer};
use rustly_common::Timestamp;
use rustly_domain::user::UserId;
use rustly_protocol::broker::TrustClass;
use rustly_protocol::BROKER_PROTOCOL_VERSION;
use rustly_storage::memory::MemoryStore;
use rustly_storage::MetadataStore;
use rustly_upload_protocol::{UploadReceipt, UploadTokens};
use serde_json::{json, Value};
use tower::ServiceExt as _;

struct Harness {
    app: axum::Router,
    user_token: String,
    user_id: UserId,
    issuer: TokenIssuer,
    upload_tokens: UploadTokens,
}

impl Harness {
    async fn new() -> Self {
        let store: Arc<dyn MetadataStore> = Arc::new(MemoryStore::new());
        let issuer = TokenIssuer::new(vec![0x11u8; 32]).unwrap();
        let upload_tokens = UploadTokens::new(vec![0x6bu8; 32]).unwrap();
        let state = AppState::new(Arc::clone(&store), issuer.clone(), "test-build")
            .with_artifact_gateway(upload_tokens.clone(), "https://artifacts.test");
        let user_id = seed::ownership_slice(&store).await.expect("seed the slice");
        let user_token = issuer.issue_user(user_id, &Scope::USER_DEFAULT, Timestamp::now(), 3600);

        Self {
            app: rustly_api::app(state),
            user_token,
            user_id,
            issuer,
            upload_tokens,
        }
    }

    fn source_receipt(&self, cid: &str) -> String {
        self.upload_tokens.sign_receipt(&UploadReceipt {
            user_id: self.user_id.to_string(),
            cid: cid.into(),
            size: 32,
            expires_at: Timestamp::now().unix_seconds() + 3600,
            nonce: uuid::Uuid::new_v4().to_string(),
        })
    }

    fn worker_token(&self, worker_id: &str, trust: TrustClass) -> String {
        self.issuer
            .issue_worker(worker_id, trust, Timestamp::now(), 3600)
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = match body {
            Some(value) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&value).unwrap()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };

        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        (status, value)
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
        self.call(Method::GET, uri, token, None).await
    }

    async fn post(&self, uri: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
        self.call(Method::POST, uri, token, Some(body)).await
    }
}

#[tokio::test]
async fn health_readiness_and_version_are_served() {
    let h = Harness::new().await;

    let (status, body) = h.get("/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");

    let (status, body) = h.get("/ready", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["checks"][0]["name"], "metadata_store");
    assert_eq!(body["checks"][0]["healthy"], true);

    let (status, body) = h.get("/api/version", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["api_version"], 1);
    assert_eq!(body["broker_protocol_version"], BROKER_PROTOCOL_VERSION);
    assert_eq!(body["ranking_model"], "provisional-v0");
    assert_eq!(
        body["ranking_provisional"], true,
        "the API must admit that the ranking model is provisional"
    );
}

#[tokio::test]
async fn guest_auth_and_upload_grants_bind_storage_to_the_user_and_cid() {
    let h = Harness::new().await;
    let (status, guest) = h.post("/api/v1/auth/guest", None, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(guest["access_token"].as_str().unwrap().starts_with("v1."));
    assert!(guest["username"].as_str().unwrap().starts_with("guest-"));

    let cid = format!("b3:{}", "a".repeat(64));
    let (status, grant) = h
        .post(
            "/api/v1/uploads/source",
            Some(&h.user_token),
            json!({"cid": cid, "size": 12}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        grant["upload_url"],
        format!("https://artifacts.test/api/v1/uploads/{cid}")
    );
    let claims = h
        .upload_tokens
        .verify_grant(
            grant["grant"].as_str().unwrap(),
            Timestamp::now().unix_seconds(),
        )
        .unwrap();
    assert_eq!(claims.user_id, h.user_id.to_string());
    assert_eq!(claims.cid, cid);
    assert_eq!(claims.size, 12);

    let (status, body) = h
        .post(
            "/api/v1/submissions",
            Some(&h.user_token),
            json!({
                "trial": "ownership-move-or-borrow",
                "source_cid": "b3:different",
                "source_receipt": h.source_receipt("b3:source"),
                "idempotency_key": "mismatch"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_request");
}

#[tokio::test]
async fn framework_failures_are_json_and_guest_auth_is_rate_limited() {
    let h = Harness::new().await;
    for attempt in 0..6 {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/guest")
            .header("cf-connecting-ip", "203.0.113.7")
            .body(Body::empty())
            .unwrap();
        let response = h.app.clone().oneshot(request).await.unwrap();
        if attempt < 5 {
            assert_eq!(response.status(), StatusCode::OK);
        } else {
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert!(response.headers().contains_key(header::RETRY_AFTER));
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["code"], "rate_limited");
        }
    }

    for (method, uri, expected) in [
        (Method::GET, "/missing", StatusCode::NOT_FOUND),
        (Method::PUT, "/health", StatusCode::METHOD_NOT_ALLOWED),
    ] {
        let response = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        assert!(response.headers().contains_key("x-request-id"));
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["request_id"].as_str().is_some());
    }

    for (content_type, body, expected, code) in [
        (
            "application/json",
            "{not-json",
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            "text/plain",
            "{}",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
        ),
    ] {
        let response = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/uploads/source")
                    .header(header::AUTHORIZATION, format!("Bearer {}", h.user_token))
                    .header(header::CONTENT_TYPE, content_type)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        let request_id = response.headers()["x-request-id"].clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], code);
        assert_eq!(body["request_id"], request_id.to_str().unwrap());
    }
}

#[tokio::test]
async fn public_profile_exposes_exactly_the_seven_permitted_fields() {
    let h = Harness::new().await;
    let (status, body) = h.get("/api/v1/users/ferris", None).await;
    assert_eq!(status, StatusCode::OK);

    let mut keys: Vec<&str> = body["profile"]
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "clan",
            "global_rank",
            "highest_trial_milestone",
            "level",
            "rank",
            "trials_solved",
            "username"
        ],
        "the public profile shape is a product decision; adding a field needs its own review"
    );

    // An unranked user has no global rank rather than a discouraging huge number.
    assert!(body["profile"]["global_rank"].is_null());
    assert!(body["profile"]["highest_trial_milestone"].is_null());
}

#[tokio::test]
async fn username_lookup_is_case_insensitive_and_missing_users_are_404() {
    let h = Harness::new().await;
    assert_eq!(h.get("/api/v1/users/FERRIS", None).await.0, StatusCode::OK);

    let (status, body) = h.get("/api/v1/users/nobody-here", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    assert!(body["request_id"].as_str().is_some_and(|s| !s.is_empty()));
}

#[tokio::test]
async fn trial_metadata_is_served_without_any_statement_or_test_payload() {
    let h = Harness::new().await;
    let (status, body) = h.get("/api/v1/trials/ownership-move-or-borrow", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["title"], "Move or Borrow?");
    assert_eq!(body["difficulty"], "easy");
    assert_eq!(body["lifecycle"], "official");
    assert_eq!(body["status"], "unsolved");
    assert!(body["content_cid"].as_str().unwrap().starts_with("b3:"));

    // Static-first: the API hands over a CID, not the content.
    let keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    for forbidden in ["statement", "starter", "tests", "solution", "hidden_tests"] {
        assert!(
            !keys.contains(&forbidden),
            "the API must not serve `{forbidden}`"
        );
    }

    let (status, _) = h.get("/api/v1/trials/does-not-exist", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn trial_listing_filters_by_difficulty_and_topic() {
    let h = Harness::new().await;

    let (_, body) = h.get("/api/v1/trials", None).await;
    assert_eq!(body["trials"].as_array().unwrap().len(), 1);

    let (_, body) = h.get("/api/v1/trials?difficulty=easy", None).await;
    assert_eq!(body["trials"].as_array().unwrap().len(), 1);

    let (_, body) = h.get("/api/v1/trials?difficulty=expert", None).await;
    assert!(body["trials"].as_array().unwrap().is_empty());

    let (_, body) = h.get("/api/v1/trials?topic=concurrency", None).await;
    assert!(body["trials"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn progress_checkpoints_are_idempotent_and_bounded() {
    let h = Harness::new().await;
    let checkpoint = json!({
        "format_version": 1,
        "device": "browser-a",
        "entries": [
            {
                "key": "learn/ownership/move-semantics",
                "revision": 3,
                "completion": "completed",
                "recorded_at": "2026-01-01T00:00:00Z"
            }
        ]
    });

    let (status, body) = h
        .post(
            "/api/v1/progress/checkpoints",
            Some(&h.user_token),
            checkpoint.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["inserted"], 1);

    // Replaying the same batch changes nothing.
    let (_, body) = h
        .post(
            "/api/v1/progress/checkpoints",
            Some(&h.user_token),
            checkpoint,
        )
        .await;
    assert_eq!(body["unchanged"], 1);
    assert_eq!(body["inserted"], 0);
    assert_eq!(body["updated"], 0);

    let (status, body) = h.get("/api/v1/progress", Some(&h.user_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["entries"]["learn/ownership/move-semantics"],
        "completed"
    );
    assert_eq!(body["completed"], 1);

    // Anonymous callers cannot write progress.
    let (status, _) = h
        .post(
            "/api/v1/progress/checkpoints",
            None,
            json!({"format_version": 1, "device": "x", "entries": []}),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_full_submit_lease_judge_loop_updates_rank_status_and_the_feed() {
    let h = Harness::new().await;

    // 1. Submit. The body carries a CID, never source.
    let submit = json!({
        "trial": "ownership-move-or-borrow",
        "source_cid": "b3:aaaabbbbccccdddd",
        "source_receipt": h.source_receipt("b3:aaaabbbbccccdddd"),
        "idempotency_key": "attempt-1"
    });
    let (status, created) = h
        .post("/api/v1/submissions", Some(&h.user_token), submit.clone())
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(created["state"], "queued");
    assert_eq!(created["idempotent_replay"], false);
    let submission_id = created["submission_id"].as_str().unwrap().to_owned();
    let job_id = created["job_id"].as_str().unwrap().to_owned();

    // 2. Replaying the idempotency key returns the same submission and job.
    let (status, replay) = h
        .post("/api/v1/submissions", Some(&h.user_token), submit)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["submission_id"], created["submission_id"]);
    assert_eq!(replay["job_id"], created["job_id"]);
    assert_eq!(replay["idempotent_replay"], true);

    // 3. A worker leases the job. This one is Trusted, so it may receive hidden tests.
    let worker_token = h.worker_token("worker-1", TrustClass::Trusted);
    let (status, lease) = h
        .post(
            "/api/v1/judge/leases",
            Some(&worker_token),
            json!({
                "protocol_version": BROKER_PROTOCOL_VERSION,
                "worker_id": "worker-1",
                "trust_class": "trusted",
                "backends": ["wasmtime"],
                "capacity": 4
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let jobs = lease["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["job_id"], job_id);
    assert_eq!(jobs[0]["may_receive_hidden_tests"], true);
    assert_eq!(jobs[0]["backend"], "wasmtime");
    let trial_package_cid = jobs[0]["trial_package_cid"].as_str().unwrap().to_owned();
    assert!(jobs[0]["limits"]["fuel"].as_u64().unwrap() > 0);
    // The lease carries identifiers, not payloads.
    assert!(!lease.to_string().contains("fn main"));

    // 4. Heartbeat.
    let (status, _) = h
        .post(
            &format!("/api/v1/judge/jobs/{job_id}/heartbeat"),
            Some(&worker_token),
            json!({"worker_id": "worker-1", "progress": {"phase": "compiling"}}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, mid) = h
        .get(
            &format!("/api/v1/submissions/{submission_id}"),
            Some(&h.user_token),
        )
        .await;
    assert_eq!(mid["state"], "compiling");

    // 5. Report an accepted verdict.
    let report = json!({
        "protocol_version": BROKER_PROTOCOL_VERSION,
        "worker_id": "worker-1",
        "trial_package_cid": trial_package_cid,
        "verdict": "AC",
        "result_manifest_hash": "b3:manifest",
        "peak_memory_bytes": 1048576,
        "execution_ms": 7,
        "compile_ms": 830,
        "used_cached_artifact": false
    });
    let mut wrong_package = report.clone();
    wrong_package["trial_package_cid"] = json!("b3:wrong-package");
    let (status, error) = h
        .post(
            &format!("/api/v1/judge/jobs/{job_id}/result"),
            Some(&worker_token),
            wrong_package,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_request");

    let (status, ack) = h
        .post(
            &format!("/api/v1/judge/jobs/{job_id}/result"),
            Some(&worker_token),
            report.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ack["accepted"], true);
    assert_eq!(ack["recorded_verdict"], "AC");

    // 6. The submission is terminal and the Trial is solved.
    let (_, done) = h
        .get(
            &format!("/api/v1/submissions/{submission_id}"),
            Some(&h.user_token),
        )
        .await;
    assert_eq!(done["state"], "finished");
    assert_eq!(done["verdict"], "AC");

    let (_, trial) = h
        .get(
            "/api/v1/trials/ownership-move-or-borrow",
            Some(&h.user_token),
        )
        .await;
    assert_eq!(trial["status"], "solved");

    // 7. Rank and level moved; the profile still shows only the permitted fields.
    let (_, me) = h.get("/api/v1/me", Some(&h.user_token)).await;
    assert_eq!(me["trials_solved"], 1);
    assert!(me["rank"].as_f64().unwrap() > 0.0);
    assert!(me["level"].as_u64().unwrap() >= 1);

    let (_, profile) = h.get("/api/v1/users/ferris", None).await;
    assert_eq!(profile["profile"]["trials_solved"], 1);
    assert_eq!(profile["profile"]["global_rank"], 1);
    // One solve is below the first milestone threshold, so no badge yet.
    assert!(profile["profile"]["highest_trial_milestone"].is_null());

    // 8. Reporting again is idempotent: no double count, no second rank move.
    let mut duplicate = report.clone();
    duplicate["verdict"] = json!("WA");
    let (_, ack) = h
        .post(
            &format!("/api/v1/judge/jobs/{job_id}/result"),
            Some(&worker_token),
            duplicate,
        )
        .await;
    assert_eq!(
        ack["accepted"], false,
        "a second report must not finalise again"
    );
    assert_eq!(ack["recorded_verdict"], "AC");

    let (_, me) = h.get("/api/v1/me", Some(&h.user_token)).await;
    assert_eq!(
        me["trials_solved"], 1,
        "trials_solved must not double-count"
    );
}

#[tokio::test]
async fn hidden_tests_are_never_dispatched_to_an_untrusted_worker() {
    let h = Harness::new().await;
    h.post(
        "/api/v1/submissions",
        Some(&h.user_token),
        json!({
            "trial": "ownership-move-or-borrow",
            "source_cid": "b3:source",
            "source_receipt": h.source_receipt("b3:source"),
            "idempotency_key": "attempt-volunteer"
        }),
    )
    .await;

    let token = h.worker_token("volunteer-1", TrustClass::Volunteer);
    let (status, lease) = h
        .post(
            "/api/v1/judge/leases",
            Some(&token),
            json!({
                "protocol_version": BROKER_PROTOCOL_VERSION,
                "worker_id": "volunteer-1",
                "trust_class": "volunteer",
                "backends": ["wasmtime"],
                "capacity": 1
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(lease["jobs"][0]["may_receive_hidden_tests"], false);
}

#[tokio::test]
async fn a_worker_cannot_claim_a_higher_trust_class_than_its_credential() {
    let h = Harness::new().await;
    let token = h.worker_token("volunteer-2", TrustClass::Volunteer);

    let (status, body) = h
        .post(
            "/api/v1/judge/leases",
            Some(&token),
            json!({
                "protocol_version": BROKER_PROTOCOL_VERSION,
                "worker_id": "volunteer-2",
                "trust_class": "trusted",
                "backends": ["wasmtime"],
                "capacity": 1
            }),
        )
        .await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a self-declared trust upgrade must be refused, not silently downgraded"
    );
    assert_eq!(body["code"], "forbidden");
}

#[tokio::test]
async fn users_cannot_use_the_broker_and_workers_cannot_read_profiles() {
    let h = Harness::new().await;

    let (status, _) = h
        .post(
            "/api/v1/judge/leases",
            Some(&h.user_token),
            json!({
                "protocol_version": BROKER_PROTOCOL_VERSION,
                "worker_id": "x",
                "trust_class": "trusted",
                "backends": ["wasmtime"],
                "capacity": 1
            }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a user token must not lease judge jobs"
    );

    let worker = h.worker_token("worker-9", TrustClass::Trusted);
    let (status, _) = h.get("/api/v1/me", Some(&worker)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a worker token must not read an account"
    );
}

#[tokio::test]
async fn a_submission_belonging_to_someone_else_is_reported_as_missing() {
    let h = Harness::new().await;
    let (_, created) = h
        .post(
            "/api/v1/submissions",
            Some(&h.user_token),
            json!({
                "trial": "ownership-move-or-borrow",
                "source_cid": "b3:source",
                "source_receipt": h.source_receipt("b3:source"),
                "idempotency_key": "attempt-privacy"
            }),
        )
        .await;
    let submission_id = created["submission_id"].as_str().unwrap();

    let stranger = h
        .issuer
        .issue_user(UserId::new(), &Scope::USER_DEFAULT, Timestamp::now(), 3600);
    let (status, body) = h
        .get(
            &format!("/api/v1/submissions/{submission_id}"),
            Some(&stranger),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another user's submission must be 404, not 403, so ids cannot be probed"
    );
    assert_eq!(body["code"], "not_found");
    assert_ne!(h.user_id, UserId::new());
}

#[tokio::test]
async fn expired_and_malformed_credentials_are_rejected() {
    let h = Harness::new().await;

    let expired = h.issuer.issue_user(
        h.user_id,
        &Scope::USER_DEFAULT,
        Timestamp::now().plus_seconds(-7200),
        3600,
    );
    assert_eq!(
        h.get("/api/v1/me", Some(&expired)).await.0,
        StatusCode::UNAUTHORIZED
    );

    assert_eq!(
        h.get("/api/v1/me", Some("garbage")).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(h.get("/api/v1/me", None).await.0, StatusCode::UNAUTHORIZED);

    let other_issuer = TokenIssuer::new(vec![0x99u8; 32]).unwrap();
    let forged = other_issuer.issue_user(h.user_id, &Scope::USER_DEFAULT, Timestamp::now(), 3600);
    assert_eq!(
        h.get("/api/v1/me", Some(&forged)).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn revealing_solutions_forfeits_rank_credit_but_still_solves() {
    let h = Harness::new().await;

    let (status, _) = h
        .post(
            "/api/v1/trials/ownership-move-or-borrow/reveal",
            Some(&h.user_token),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (_, created) = h
        .post(
            "/api/v1/submissions",
            Some(&h.user_token),
            json!({
                "trial": "ownership-move-or-borrow",
                "source_cid": "b3:source",
                "source_receipt": h.source_receipt("b3:source"),
                "idempotency_key": "after-reveal"
            }),
        )
        .await;
    let job_id = created["job_id"].as_str().unwrap().to_owned();

    let worker_token = h.worker_token("worker-r", TrustClass::Trusted);
    let (_, lease) = h
        .post(
            "/api/v1/judge/leases",
            Some(&worker_token),
            json!({
                "protocol_version": BROKER_PROTOCOL_VERSION,
                "worker_id": "worker-r",
                "trust_class": "trusted",
                "backends": ["wasmtime"],
                "capacity": 4
            }),
        )
        .await;
    let trial_package_cid = lease["jobs"][0]["trial_package_cid"]
        .as_str()
        .unwrap()
        .to_owned();

    h.post(
        &format!("/api/v1/judge/jobs/{job_id}/result"),
        Some(&worker_token),
        json!({
            "protocol_version": BROKER_PROTOCOL_VERSION,
            "worker_id": "worker-r",
            "trial_package_cid": trial_package_cid,
            "verdict": "AC",
            "result_manifest_hash": "b3:m",
            "peak_memory_bytes": 1024,
            "execution_ms": 3,
            "compile_ms": 500,
            "used_cached_artifact": false
        }),
    )
    .await;

    let (_, me) = h.get("/api/v1/me", Some(&h.user_token)).await;
    assert_eq!(me["trials_solved"], 1, "the solve still counts");
    assert_eq!(
        me["rank"].as_f64().unwrap(),
        0.0,
        "but it earns no rank credit"
    );
}

#[tokio::test]
async fn an_infrastructure_failure_is_never_reported_as_the_users_fault() {
    let h = Harness::new().await;
    let (_, created) = h
        .post(
            "/api/v1/submissions",
            Some(&h.user_token),
            json!({
                "trial": "ownership-move-or-borrow",
                "source_cid": "b3:source",
                "source_receipt": h.source_receipt("b3:source"),
                "idempotency_key": "infra"
            }),
        )
        .await;
    let job_id = created["job_id"].as_str().unwrap().to_owned();

    let worker_token = h.worker_token("worker-i", TrustClass::Trusted);
    let (_, lease) = h
        .post(
            "/api/v1/judge/leases",
            Some(&worker_token),
            json!({
                "protocol_version": BROKER_PROTOCOL_VERSION,
                "worker_id": "worker-i",
                "trust_class": "trusted",
                "backends": ["wasmtime"],
                "capacity": 4
            }),
        )
        .await;
    let trial_package_cid = lease["jobs"][0]["trial_package_cid"]
        .as_str()
        .unwrap()
        .to_owned();

    let (_, ack) = h
        .post(
            &format!("/api/v1/judge/jobs/{job_id}/result"),
            Some(&worker_token),
            json!({
                "protocol_version": BROKER_PROTOCOL_VERSION,
                "worker_id": "worker-i",
                "trial_package_cid": trial_package_cid,
                "verdict": "IE",
                "result_manifest_hash": "b3:m",
                "peak_memory_bytes": 0,
                "execution_ms": 0,
                "compile_ms": 0,
                "used_cached_artifact": false
            }),
        )
        .await;
    assert_eq!(ack["recorded_verdict"], "IE");

    let (_, trial) = h
        .get(
            "/api/v1/trials/ownership-move-or-borrow",
            Some(&h.user_token),
        )
        .await;
    assert_eq!(
        trial["status"], "unsolved",
        "an infrastructure failure must not mark the Trial attempted"
    );
}

#[tokio::test]
async fn the_recent_feed_reports_publication_not_activity() {
    let h = Harness::new().await;
    let (status, body) = h.get("/api/v1/recent", None).await;
    assert_eq!(status, StatusCode::OK);

    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"], "trial_published");
    assert_eq!(events[0]["href"], "/trials/ownership-move-or-borrow");

    let (status, archive) = h.get("/api/v1/archive?kind=lesson_published", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(archive["events"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn oversized_bodies_are_refused_so_source_cannot_travel_through_the_api() {
    let h = Harness::new().await;
    let huge = "a".repeat(rustly_api::routes::MAX_BODY_BYTES + 1024);
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/progress/checkpoints")
        .header(header::AUTHORIZATION, format!("Bearer {}", h.user_token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(huge))
        .unwrap();

    let response = h.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let request_id = response.headers()["x-request-id"].clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "payload_too_large");
    assert_eq!(body["request_id"], request_id.to_str().unwrap());
}

#[tokio::test]
async fn a_caller_supplied_request_id_is_sanitised_before_it_is_echoed() {
    let h = Harness::new().await;
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/users/nobody")
        .header("x-request-id", "trace-abc-123")
        .body(Body::empty())
        .unwrap();
    let response = h.app.clone().oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["request_id"], "trace-abc-123");

    let hostile = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/users/nobody")
        .header("x-request-id", "injected\tvalue")
        .body(Body::empty())
        .unwrap();
    let response = h.app.clone().oneshot(hostile).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_ne!(body["request_id"], "injected\tvalue");
}
