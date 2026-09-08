//! Shared primitives for the Rustly control plane.
//!
//! This crate is deliberately tiny. It holds the things every other crate needs
//! (errors, identifiers, time, telemetry) and nothing that encodes product
//! behaviour. Product behaviour belongs in `rustly-domain`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod id;
pub mod telemetry;
pub mod time;

pub use error::{Error, Result};
pub use id::Id;
pub use time::Timestamp;
