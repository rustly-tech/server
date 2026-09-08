//! Structured tracing setup.
//!
//! Rustly emits structured events from the first commit. The identifiers listed
//! in [`fields`] are the ones we correlate on across the control plane, the
//! judge, and the data plane; they are named here so every crate spells them the
//! same way.

/// Canonical span/field names. Use these constants, never string literals.
pub mod fields {
    /// Per-request correlation id, echoed back in the `x-request-id` header.
    pub const REQUEST_ID: &str = "request_id";
    /// Privacy-safe reference to the acting user.
    pub const USER_REF: &str = "user_ref";
    /// Submission identifier.
    pub const SUBMISSION_ID: &str = "submission_id";
    /// Judge job identifier.
    pub const JOB_ID: &str = "job_id";
    /// Judge worker identifier.
    pub const WORKER_ID: &str = "worker_id";
    /// Trial identifier.
    pub const TRIAL_ID: &str = "trial_id";
    /// Trial content version, so a verdict can be traced to exact tests.
    pub const TRIAL_VERSION: &str = "trial_version";
}

/// Initialise tracing from `RUST_LOG`, defaulting to `info`.
///
/// `json` selects machine-readable output for deployed environments; human
/// output is used locally. Returns an error if a subscriber is already set,
/// which is how repeated initialisation in tests is caught.
pub fn init(json: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let registry = tracing_subscriber::registry().with(filter);

    if json {
        registry
            .with(tracing_subscriber::fmt::layer().json().flatten_event(true))
            .try_init()?;
    } else {
        registry
            .with(tracing_subscriber::fmt::layer().compact())
            .try_init()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::fields;

    #[test]
    fn correlation_field_names_are_stable() {
        // These names appear in dashboards and log queries. Changing one is a
        // breaking observability change, so pin them with a test.
        assert_eq!(fields::REQUEST_ID, "request_id");
        assert_eq!(fields::SUBMISSION_ID, "submission_id");
        assert_eq!(fields::JOB_ID, "job_id");
        assert_eq!(fields::WORKER_ID, "worker_id");
        assert_eq!(fields::TRIAL_ID, "trial_id");
        assert_eq!(fields::TRIAL_VERSION, "trial_version");
        assert_eq!(fields::USER_REF, "user_ref");
    }
}
