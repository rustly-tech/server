//! Versioned wire contracts.
//!
//! Everything a client or a judge worker can see on the wire is defined here, in
//! one crate, so that a breaking change is a visible diff in a single place.
//!
//! Two independently versioned surfaces live here:
//!
//! | Surface | Version constant | Path |
//! | --- | --- | --- |
//! | Public HTTP API | [`API_VERSION`] | `/api/v1/*` |
//! | Judge broker protocol | [`BROKER_PROTOCOL_VERSION`] | `/api/v1/judge/*` |
//!
//! ## What never crosses this boundary
//!
//! * **Blobs.** Source, test data, and compiled artifacts are content-addressed
//!   and fetched from the data plane. The API carries CIDs and locations only.
//! * **Hidden tests.** They are never serialised into any type in this crate.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod api;
pub mod broker;

/// Major version of the public HTTP API, matching the `/api/v1` path segment.
pub const API_VERSION: u32 = 1;

/// Version of the judge broker protocol (lease / heartbeat / report).
pub const BROKER_PROTOCOL_VERSION: u32 = 1;
