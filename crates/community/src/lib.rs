//! Community surfaces.
//!
//! Two things live here:
//!
//! * **The event feed** ([`feed`]) - Recent is a 24-hour window over meaningful
//!   events, Archive is the same log without the window. Neither is an activity
//!   feed: the event kinds are a closed, curated set in `rustly-domain`.
//! * **Q&A types** ([`qa`]) - validated question and answer types.
//!   Status: `IMPLEMENTED` for the types and validation, `PLANNED` for
//!   persistence and moderation, which are tracked as a P2 epic.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod feed;
pub mod qa;
