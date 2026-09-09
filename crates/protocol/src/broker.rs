//! Judge broker protocol (`/api/v1/judge/*`).
//!
//! This is the contract between the trusted control plane and judge workers. It
//! is a **pull** protocol: workers lease jobs, heartbeat while working, and
//! report a result. The control plane never pushes to a worker and never
//! executes anything itself.
//!
//! # Trust classes
//!
//! A worker declares a [`TrustClass`]. The control plane uses it to decide what
//! a worker is allowed to see:
//!
//! * [`TrustClass::Trusted`] - operator-run. May receive hidden tests.
//! * [`TrustClass::Community`] - known operator, not fully trusted. Public tests only.
//! * [`TrustClass::Volunteer`] - anonymous capacity. Public tests only; its
//!   compiled artifacts are quarantined until independently verified.
//!
//! **Hidden tests are never dispatched to a non-`Trusted` worker.** That rule is
//! encoded in [`JobLease::may_receive_hidden_tests`] and enforced by tests here
//! and in the API service.

use rustly_common::Timestamp;
use rustly_domain::Verdict;
use serde::{Deserialize, Serialize};

/// How much a worker is trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustClass {
    /// Anonymous donated capacity. Least trusted.
    Volunteer,
    /// Known community operator.
    Community,
    /// Operator-run infrastructure.
    Trusted,
}

impl TrustClass {
    /// Whether workers in this class may receive hidden test material.
    ///
    /// Only `Trusted`. This is the single definition of that rule.
    pub const fn may_receive_hidden_tests(self) -> bool {
        matches!(self, Self::Trusted)
    }

    /// Whether artifacts produced by this class must be quarantined until they
    /// are independently reproduced.
    pub const fn requires_artifact_quarantine(self) -> bool {
        !matches!(self, Self::Trusted)
    }
}

/// Execution backend a worker offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackend {
    /// Wasmtime + WASI. The only backend qualified for untrusted submissions.
    Wasmtime,
    /// Native process isolation. EXPERIMENTAL, qualification-gated.
    Native,
}

/// Request body of `POST /api/v1/judge/leases`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRequest {
    /// Protocol version the worker speaks.
    pub protocol_version: u32,
    /// Stable worker identifier.
    pub worker_id: String,
    /// Declared trust class. The control plane verifies this against the
    /// worker's credential; a self-declared upgrade is rejected.
    pub trust_class: TrustClass,
    /// Backends this worker can run.
    pub backends: Vec<ExecutionBackend>,
    /// How many jobs the worker wants. Bounded server-side.
    pub capacity: u32,
}

/// A leased job.
///
/// Contains no source and no test data: only content identifiers the worker
/// resolves through the data plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLease {
    /// Protocol version of this payload.
    pub protocol_version: u32,
    /// Immutable job identifier.
    pub job_id: String,
    /// CID of the submitted source.
    pub source_cid: String,
    /// CID of the Trial package: limits, public tests, checker configuration.
    pub trial_package_cid: String,
    /// Trial content version, for verdict traceability.
    pub trial_version: u32,
    /// Environment identifier: Rust edition, toolchain, target triple.
    pub environment_id: String,
    /// Resource limits the worker must enforce.
    pub limits: ExecutionLimits,
    /// Backend the worker must use for this job.
    pub backend: ExecutionBackend,
    /// Whether hidden tests are included in the referenced package.
    ///
    /// Always `false` unless the leasing worker is [`TrustClass::Trusted`].
    pub may_receive_hidden_tests: bool,
    /// When the lease expires. After this the job is re-queued.
    pub lease_expires_at: Timestamp,
}

/// Response of `POST /api/v1/judge/leases`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseResponse {
    /// Leased jobs, possibly empty.
    pub jobs: Vec<JobLease>,
    /// Seconds the worker should wait before asking again when the queue is empty.
    pub poll_after_seconds: u32,
}

/// Resource limits enforced by the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionLimits {
    /// Wall-clock limit for compilation, milliseconds.
    pub compile_wall_ms: u64,
    /// Wall-clock limit per test execution, milliseconds.
    pub run_wall_ms: u64,
    /// Memory limit per execution, bytes.
    pub memory_bytes: u64,
    /// Combined stdout+stderr limit per execution, bytes.
    pub output_bytes: u64,
    /// Abstract work bound (Wasmtime fuel), independent of host speed.
    ///
    /// Wall-clock alone is not a fair limit across heterogeneous workers.
    pub fuel: u64,
}

impl Default for ExecutionLimits {
    /// Conservative defaults used by the vertical slice.
    fn default() -> Self {
        Self {
            compile_wall_ms: 20_000,
            run_wall_ms: 2_000,
            memory_bytes: 256 * 1024 * 1024,
            output_bytes: 256 * 1024,
            fuel: 2_000_000_000,
        }
    }
}

/// Request body of `POST /api/v1/judge/jobs/{job_id}/heartbeat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    /// Worker holding the lease.
    pub worker_id: String,
    /// Coarse progress, for the submission SSE stream.
    pub progress: JobProgress,
}

/// Coarse job progress reported by a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "phase")]
pub enum JobProgress {
    /// Fetching source and package from the data plane.
    Fetching,
    /// Compiling in the compile sandbox.
    Compiling,
    /// Executing tests in the runtime sandbox.
    Running {
        /// Tests completed.
        completed: u32,
        /// Total tests dispatched.
        total: u32,
    },
}

/// Request body of `POST /api/v1/judge/jobs/{job_id}/result`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultReport {
    /// Protocol version of this payload.
    pub protocol_version: u32,
    /// Worker reporting.
    pub worker_id: String,
    /// Immutable Trial package that produced this result.
    pub trial_package_cid: String,
    /// Final verdict.
    pub verdict: Verdict,
    /// BLAKE3 hash of the full result manifest stored in the data plane.
    ///
    /// The manifest holds per-test outcomes and diagnostics. Only its hash
    /// travels through the control plane.
    pub result_manifest_hash: String,
    /// Peak memory observed, bytes.
    pub peak_memory_bytes: u64,
    /// Total execution time across tests, milliseconds.
    pub execution_ms: u64,
    /// Compilation time, milliseconds.
    pub compile_ms: u64,
    /// Whether a cached compilation artifact was reused.
    ///
    /// Reported for cache-hit metrics. It never affects the verdict.
    pub used_cached_artifact: bool,
}

/// Response of a result report. Reporting is idempotent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultAck {
    /// Whether this report was the one that finalised the submission.
    pub accepted: bool,
    /// Verdict now recorded, which may differ from the report if another worker
    /// finalised the job first.
    pub recorded_verdict: Verdict,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_trusted_workers_may_receive_hidden_tests() {
        assert!(TrustClass::Trusted.may_receive_hidden_tests());
        assert!(!TrustClass::Community.may_receive_hidden_tests());
        assert!(!TrustClass::Volunteer.may_receive_hidden_tests());
    }

    #[test]
    fn untrusted_artifacts_are_quarantined() {
        assert!(TrustClass::Volunteer.requires_artifact_quarantine());
        assert!(TrustClass::Community.requires_artifact_quarantine());
        assert!(!TrustClass::Trusted.requires_artifact_quarantine());
    }

    #[test]
    fn trust_classes_are_ordered_least_to_most_trusted() {
        assert!(TrustClass::Volunteer < TrustClass::Community);
        assert!(TrustClass::Community < TrustClass::Trusted);
    }

    #[test]
    fn a_lease_carries_identifiers_not_payloads() {
        let lease = JobLease {
            protocol_version: crate::BROKER_PROTOCOL_VERSION,
            job_id: "job-1".into(),
            source_cid: "b3:aa".into(),
            trial_package_cid: "b3:bb".into(),
            trial_version: 3,
            environment_id: "rust-1.85-wasm32-wasip1".into(),
            limits: ExecutionLimits::default(),
            backend: ExecutionBackend::Wasmtime,
            may_receive_hidden_tests: false,
            lease_expires_at: Timestamp::now(),
        };
        let value = serde_json::to_value(&lease).unwrap();
        let blob = value.to_string();
        assert!(blob.contains("b3:aa"), "CIDs are carried");
        assert!(!blob.contains("fn main"), "source never is");
    }

    #[test]
    fn default_limits_bound_every_dimension() {
        let l = ExecutionLimits::default();
        for (name, value) in [
            ("compile_wall_ms", l.compile_wall_ms),
            ("run_wall_ms", l.run_wall_ms),
            ("memory_bytes", l.memory_bytes),
            ("output_bytes", l.output_bytes),
            ("fuel", l.fuel),
        ] {
            assert!(value > 0, "{name} must be bounded");
        }
    }

    #[test]
    fn progress_is_tagged_by_phase() {
        let value = serde_json::to_value(JobProgress::Running {
            completed: 3,
            total: 7,
        })
        .unwrap();
        assert_eq!(value["phase"], "running");
        assert_eq!(value["completed"], 3);
    }

    #[test]
    fn cache_reuse_is_metrics_only_and_carries_no_verdict_authority() {
        let report = ResultReport {
            protocol_version: crate::BROKER_PROTOCOL_VERSION,
            worker_id: "w1".into(),
            trial_package_cid: "b3:trial-package".into(),
            verdict: Verdict::Accepted,
            result_manifest_hash: "b3:cc".into(),
            peak_memory_bytes: 1024,
            execution_ms: 12,
            compile_ms: 900,
            used_cached_artifact: true,
        };
        let mut cold = report.clone();
        cold.used_cached_artifact = false;
        assert_eq!(report.verdict, cold.verdict);
    }
}
