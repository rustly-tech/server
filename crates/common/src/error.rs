//! The single error type crossing Rustly control-plane crate boundaries.
//!
//! Library crates return [`Error`]. The API service maps it to a status code in
//! exactly one place, so a new variant cannot silently become a 500.

use std::fmt;

/// Convenience alias for fallible control-plane operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A control-plane failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The requested entity does not exist, or the caller may not know it does.
    #[error("{kind} not found: {key}")]
    NotFound {
        /// Entity kind, e.g. `"trial"`.
        kind: &'static str,
        /// Lookup key that produced no result.
        key: String,
    },

    /// The request was structurally valid but semantically wrong.
    #[error("invalid {field}: {reason}")]
    Invalid {
        /// Offending field.
        field: &'static str,
        /// Human-readable reason.
        reason: String,
    },

    /// The operation conflicts with existing state.
    #[error("conflict: {0}")]
    Conflict(String),

    /// No credentials, or credentials that did not verify.
    #[error("unauthenticated: {0}")]
    Unauthenticated(String),

    /// Authenticated, but not permitted.
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// A dependency (database, queue, upstream) failed.
    ///
    /// This is deliberately distinct from [`Error::Invalid`]: an infrastructure
    /// failure must never be reported to a user as if they had made a mistake.
    #[error("dependency `{dependency}` failed: {source}")]
    Dependency {
        /// Which dependency failed.
        dependency: &'static str,
        /// Underlying cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A bug in Rustly.
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Build a [`Error::NotFound`].
    pub fn not_found(kind: &'static str, key: impl fmt::Display) -> Self {
        Self::NotFound {
            kind,
            key: key.to_string(),
        }
    }

    /// Build an [`Error::Invalid`].
    pub fn invalid(field: &'static str, reason: impl fmt::Display) -> Self {
        Self::Invalid {
            field,
            reason: reason.to_string(),
        }
    }

    /// Build an [`Error::Dependency`].
    pub fn dependency<E>(dependency: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Dependency {
            dependency,
            source: Box::new(source),
        }
    }

    /// Stable machine-readable code for API responses and metrics.
    ///
    /// These strings are part of the public API contract. Adding is fine;
    /// renaming is a breaking change.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "not_found",
            Self::Invalid { .. } => "invalid_request",
            Self::Conflict(_) => "conflict",
            Self::Unauthenticated(_) => "unauthenticated",
            Self::Forbidden(_) => "forbidden",
            Self::Dependency { .. } => "dependency_unavailable",
            Self::Internal(_) => "internal",
        }
    }

    /// Whether the message is safe to show a caller verbatim.
    ///
    /// Dependency and internal errors can carry connection strings, table names,
    /// or stack context. They are logged in full and summarised to the caller.
    pub fn is_client_safe(&self) -> bool {
        matches!(
            self,
            Self::NotFound { .. }
                | Self::Invalid { .. }
                | Self::Conflict(_)
                | Self::Unauthenticated(_)
                | Self::Forbidden(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_and_internal_errors_are_not_client_safe() {
        let io = std::io::Error::other("connection refused to 10.0.0.4:5432");
        let err = Error::dependency("postgres", io);
        assert!(!err.is_client_safe());
        assert_eq!(err.code(), "dependency_unavailable");

        assert!(!Error::Internal("unreachable branch".into()).is_client_safe());
    }

    #[test]
    fn user_facing_errors_are_client_safe() {
        assert!(Error::not_found("trial", "ownership-move").is_client_safe());
        assert!(Error::invalid("username", "too short").is_client_safe());
        assert!(Error::Conflict("already solved".into()).is_client_safe());
    }

    #[test]
    fn codes_are_stable_strings() {
        assert_eq!(Error::not_found("trial", "x").code(), "not_found");
        assert_eq!(Error::invalid("f", "r").code(), "invalid_request");
        assert_eq!(
            Error::Unauthenticated("no token".into()).code(),
            "unauthenticated"
        );
        assert_eq!(Error::Forbidden("scope".into()).code(), "forbidden");
    }
}
