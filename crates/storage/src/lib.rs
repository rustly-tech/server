//! Persistence for the Rustly control plane.
//!
//! # Why an abstraction rather than "just use sqlx everywhere"
//!
//! Invariant J says no provider may become logically irreplaceable. Domain logic
//! therefore talks to [`MetadataStore`], and the choice of PostgreSQL - Neon or
//! otherwise - is a deployment decision made in one place.
//!
//! It also buys something more immediately useful: the entire API can be tested
//! against [`memory::MemoryStore`] with no database, so `cargo test` is fast,
//! hermetic, and works on a laptop with no containers running.
//!
//! # What is stored here, and what is not
//!
//! This store holds **small, mutable, authoritative** state: identity, ranking,
//! refs to content, submission metadata. It never holds source code, test data,
//! compiled artifacts, or any other large immutable object. Those live in the
//! content-addressed data plane and are referenced by CID.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "testing")]
pub mod conformance;
pub mod finalize;
#[cfg(feature = "memory")]
pub mod memory;
#[cfg(feature = "postgres")]
pub mod postgres;
mod store;

pub use store::{
    LeasedJob, MetadataStore, NewSubmission, SubmissionOutcome, TrialFilter, TrialUserState,
};
