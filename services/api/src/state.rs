//! Shared application state.

use std::sync::Arc;

use rustly_auth::TokenIssuer;
use rustly_common::Result;
use rustly_protocol::api::SubmissionEvent;
use rustly_ranking::{ProvisionalV0, RankingModel};
use rustly_storage::memory::MemoryStore;
use rustly_storage::MetadataStore;
use rustly_upload_protocol::UploadTokens;
use tokio::sync::broadcast;

use crate::rate_limit::RateLimiter;

/// Capacity of the submission event bus.
///
/// A slow SSE client that falls this far behind is disconnected rather than
/// allowed to grow an unbounded buffer. It can re-read state with a plain `GET`.
const EVENT_BUS_CAPACITY: usize = 1024;

/// Everything a handler needs.
#[derive(Clone)]
pub struct AppState {
    /// The metadata store.
    pub store: Arc<dyn MetadataStore>,
    /// Token issuer and verifier.
    pub tokens: Arc<TokenIssuer>,
    /// Upload grant and receipt signer, shared with the artifact gateway.
    pub upload_tokens: Arc<UploadTokens>,
    /// Browser-visible artifact gateway base URL.
    pub artifact_gateway_url: String,
    /// The ranking model in force. Behind a trait so it stays replaceable.
    pub ranking: Arc<dyn RankingModel>,
    /// Build identifier.
    pub build: String,
    /// Submission state transitions, fanned out to SSE subscribers.
    pub events: broadcast::Sender<SubmissionEvent>,
    /// Basic per-process public endpoint rate limits.
    pub rate_limits: RateLimiter,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("ranking", &self.ranking.id())
            .field("build", &self.build)
            .finish_non_exhaustive()
    }
}

impl AppState {
    /// Build state around a store.
    pub fn new(
        store: Arc<dyn MetadataStore>,
        tokens: TokenIssuer,
        build: impl Into<String>,
    ) -> Self {
        let (events, _) = broadcast::channel(EVENT_BUS_CAPACITY);
        Self {
            store,
            tokens: Arc::new(tokens),
            upload_tokens: Arc::new(
                UploadTokens::new(vec![0x6b; 32]).expect("static development key is valid"),
            ),
            artifact_gateway_url: "http://127.0.0.1:8081".into(),
            ranking: Arc::new(ProvisionalV0),
            build: build.into(),
            events,
            rate_limits: RateLimiter::default(),
        }
    }

    /// Configure the storage capability boundary.
    pub fn with_artifact_gateway(
        mut self,
        tokens: UploadTokens,
        base_url: impl Into<String>,
    ) -> Self {
        self.upload_tokens = Arc::new(tokens);
        self.artifact_gateway_url = base_url.into().trim_end_matches('/').to_owned();
        self
    }

    /// An in-memory state for tests and local development.
    pub fn in_memory() -> Result<Self> {
        let tokens = TokenIssuer::new(vec![0x5au8; 32])?;
        Ok(Self::new(Arc::new(MemoryStore::new()), tokens, "test"))
    }

    /// Publish a submission transition. Failure means nobody is listening.
    pub fn publish(&self, event: SubmissionEvent) {
        let _ = self.events.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_does_not_expose_the_token_issuer_secret() {
        let state = AppState::in_memory().unwrap();
        let text = format!("{state:?}");
        assert!(text.contains("provisional-v0"));
        assert!(!text.contains("secret"));
    }

    #[test]
    fn publishing_with_no_subscribers_is_not_an_error() {
        let state = AppState::in_memory().unwrap();
        state.publish(SubmissionEvent {
            submission_id: rustly_domain::SubmissionId::new(),
            state: rustly_domain::SubmissionState::Queued,
            at: rustly_common::Timestamp::now(),
        });
    }
}
