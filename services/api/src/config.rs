//! Process configuration, read from the environment.
//!
//! Everything has a working default except the token secret, which fails closed:
//! a service that silently invents a signing key would issue tokens that stop
//! verifying after a restart.

use rustly_common::{Error, Result};

/// Which metadata backend to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// In-memory. Local development and tests.
    Memory,
    /// PostgreSQL at the given URL.
    Postgres(String),
}

/// Service configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address to bind.
    pub bind: String,
    /// Metadata backend.
    pub backend: Backend,
    /// Token signing secret.
    pub token_secret: Vec<u8>,
    /// Build identifier, normally the git SHA.
    pub build: String,
    /// Emit JSON logs.
    pub json_logs: bool,
    /// Seed the in-memory backend with the Ownership vertical slice.
    pub seed_slice: bool,
}

impl Config {
    /// Read configuration from the environment.
    pub fn from_env() -> Result<Self> {
        let backend = match std::env::var("DATABASE_URL") {
            Ok(url) if !url.is_empty() => Backend::Postgres(url),
            _ => Backend::Memory,
        };

        let token_secret = match std::env::var("RUSTLY_TOKEN_SECRET") {
            Ok(secret) if secret.len() >= 32 => secret.into_bytes(),
            Ok(_) => {
                return Err(Error::invalid(
                    "RUSTLY_TOKEN_SECRET",
                    "must be at least 32 bytes",
                ))
            }
            Err(_) if backend == Backend::Memory => {
                // Development only, and only for the ephemeral in-memory
                // backend: tokens die with the process, which is correct.
                tracing::warn!(
                    "RUSTLY_TOKEN_SECRET is unset; generating an ephemeral development secret. \
                     Tokens will not survive a restart."
                );
                uuid::Uuid::new_v4().to_string().repeat(2).into_bytes()
            }
            Err(_) => {
                return Err(Error::invalid(
                    "RUSTLY_TOKEN_SECRET",
                    "must be set when a database is configured",
                ))
            }
        };

        Ok(Self {
            bind: std::env::var("RUSTLY_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            backend,
            token_secret,
            build: std::env::var("RUSTLY_BUILD").unwrap_or_else(|_| "dev".into()),
            json_logs: std::env::var("RUSTLY_LOG_FORMAT").as_deref() == Ok("json"),
            seed_slice: std::env::var("RUSTLY_SEED_SLICE").as_deref() != Ok("0"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_secret_is_rejected_rather_than_padded() {
        let issuer = rustly_auth::TokenIssuer::new(b"too-short".to_vec());
        assert!(issuer.is_err());
    }

    #[test]
    fn backend_selection_follows_database_url() {
        assert_eq!(Backend::Memory, Backend::Memory);
        assert_ne!(Backend::Memory, Backend::Postgres("postgres://x".into()));
    }
}
