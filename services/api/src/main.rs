//! The Rustly control-plane API binary.

use std::sync::Arc;

use anyhow::Context as _;
use rustly_api::config::{Backend, Config};
use rustly_api::AppState;
use rustly_auth::TokenIssuer;
use rustly_storage::memory::MemoryStore;
use rustly_storage::MetadataStore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env().context("reading configuration")?;
    rustly_common::telemetry::init(config.json_logs)
        .map_err(|e| anyhow::anyhow!("initialising telemetry: {e}"))?;

    let store: Arc<dyn MetadataStore> = match &config.backend {
        Backend::Memory => {
            tracing::warn!(
                "using the in-memory metadata store; all state is lost on restart. \
                 Set DATABASE_URL for a durable backend."
            );
            Arc::new(MemoryStore::new())
        }
        #[cfg(feature = "postgres")]
        Backend::Postgres(url) => {
            let store = rustly_storage::postgres::PostgresStore::connect(url, 16)
                .await
                .context("connecting to PostgreSQL")?;
            store.migrate().await.context("applying migrations")?;
            tracing::info!("connected to PostgreSQL and applied migrations");
            Arc::new(store)
        }
        #[cfg(not(feature = "postgres"))]
        Backend::Postgres(_) => anyhow::bail!(
            "DATABASE_URL is set but this binary was built without the `postgres` feature"
        ),
    };

    let tokens = TokenIssuer::new(config.token_secret.clone())
        .map_err(|e| anyhow::anyhow!("token issuer: {e}"))?;
    let state = AppState::new(Arc::clone(&store), tokens, config.build.clone());

    if config.seed_slice && matches!(config.backend, Backend::Memory) {
        match rustly_api::seed::ownership_slice(&store).await {
            Ok(user_id) => tracing::info!(%user_id, "seeded the Ownership vertical slice"),
            Err(error) => tracing::warn!(%error, "failed to seed the vertical slice"),
        }
    }

    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;
    tracing::info!(bind = %config.bind, build = %config.build, "rustly-api listening");

    axum::serve(listener, rustly_api::app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving")?;
    Ok(())
}

/// Wait for SIGINT or SIGTERM so in-flight requests finish before exit.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("installing the Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("installing the SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("received SIGINT, shutting down"),
        () = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}
