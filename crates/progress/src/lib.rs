//! Local-first learning progress.
//!
//! # The contract
//!
//! The browser is the primary writer. It accumulates progress locally (OPFS /
//! IndexedDB) and pushes **compact checkpoints** to the control plane. The
//! control plane never sees keystrokes, editor heartbeats, lesson views, or
//! individual quiz clicks.
//!
//! Because a client may be offline, retry, or run on several devices, the merge
//! must be:
//!
//! * **idempotent** - replaying the same checkpoint changes nothing;
//! * **commutative** - checkpoints may arrive out of order;
//! * **monotonic** - progress never goes backwards, even from a stale device.
//!
//! That is achieved with a per-key `revision` counter plus a monotone
//! `completion` lattice, rather than last-write-wins on wall-clock time, which
//! would let a device with a skewed clock erase real progress.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeMap;

use rustly_common::{Error, Result, Timestamp};
use serde::{Deserialize, Serialize};

/// Wire-format version for checkpoint payloads.
pub const CHECKPOINT_FORMAT_VERSION: u32 = 1;

/// Maximum number of entries a single checkpoint batch may carry.
///
/// A bound is part of the API-light invariant: without it, a client could stream
/// per-keystroke state to the control plane one "checkpoint" at a time.
pub const MAX_ENTRIES_PER_BATCH: usize = 256;

/// How complete one unit of learning is.
///
/// This is a lattice, ordered `NotStarted < Started < Completed < Mastered`.
/// Merging takes the maximum, so a stale device can never demote progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completion {
    /// No recorded interaction.
    NotStarted,
    /// Opened and partially worked.
    Started,
    /// All required steps done.
    Completed,
    /// Completed, plus the optional deepening exercises.
    Mastered,
}

/// One progress entry: the state of a single lesson, quiz, or Trial checkpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Content key, e.g. `learn/ownership/move-semantics`.
    pub key: String,
    /// Client-side revision counter for this key. Strictly increasing per device.
    pub revision: u64,
    /// Completion state.
    pub completion: Completion,
    /// When the client recorded it. Advisory only; never used to resolve conflicts.
    pub recorded_at: Timestamp,
}

impl Entry {
    /// Validate a client-supplied entry.
    pub fn validate(&self) -> Result<()> {
        if self.key.is_empty() || self.key.len() > 200 {
            return Err(Error::invalid("key", "must be 1-200 characters"));
        }
        if !self
            .key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '/' | '.'))
        {
            return Err(Error::invalid(
                "key",
                "may contain only a-z, 0-9, '-', '/' and '.'",
            ));
        }
        Ok(())
    }
}

/// A batch of checkpoints pushed by one client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Format version, so the server can reject or upgrade old clients.
    pub format_version: u32,
    /// Stable per-device identifier, for diagnosing divergence.
    pub device: String,
    /// The entries.
    pub entries: Vec<Entry>,
}

impl Checkpoint {
    /// Validate the batch shape before it touches storage.
    pub fn validate(&self) -> Result<()> {
        if self.format_version != CHECKPOINT_FORMAT_VERSION {
            return Err(Error::invalid(
                "format_version",
                format!(
                    "expected {CHECKPOINT_FORMAT_VERSION}, got {}",
                    self.format_version
                ),
            ));
        }
        if self.device.is_empty() || self.device.len() > 64 {
            return Err(Error::invalid("device", "must be 1-64 characters"));
        }
        if self.entries.is_empty() {
            return Err(Error::invalid("entries", "must not be empty"));
        }
        if self.entries.len() > MAX_ENTRIES_PER_BATCH {
            return Err(Error::invalid(
                "entries",
                format!("at most {MAX_ENTRIES_PER_BATCH} entries per batch"),
            ));
        }
        for entry in &self.entries {
            entry.validate()?;
        }
        Ok(())
    }
}

/// The server-side merged view of a user's progress.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProgressState {
    /// Merged entries, keyed by content key. `BTreeMap` so serialisation is
    /// deterministic and diffs are readable.
    pub entries: BTreeMap<String, MergedEntry>,
}

/// A merged entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MergedEntry {
    /// Highest revision seen for this key, across all devices.
    pub revision: u64,
    /// Highest completion seen for this key.
    pub completion: Completion,
    /// Most recent client timestamp seen. Advisory.
    pub recorded_at: Timestamp,
}

/// What a merge changed. Returned so the API can answer "was this a no-op?"
/// without a second read, and so metrics can distinguish real sync from retries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MergeOutcome {
    /// Keys created.
    pub inserted: usize,
    /// Keys whose merged value changed.
    pub updated: usize,
    /// Keys where the incoming entry carried nothing new.
    pub unchanged: usize,
}

impl MergeOutcome {
    /// Whether the merge changed any stored state.
    pub const fn changed(&self) -> bool {
        self.inserted > 0 || self.updated > 0
    }
}

impl ProgressState {
    /// Merge a validated checkpoint into this state.
    ///
    /// Both `revision` and `completion` move only upward, independently. Taking
    /// the max of each is what makes the operation idempotent and commutative.
    pub fn merge(&mut self, checkpoint: &Checkpoint) -> MergeOutcome {
        let mut outcome = MergeOutcome::default();
        for entry in &checkpoint.entries {
            match self.entries.get_mut(&entry.key) {
                None => {
                    self.entries.insert(
                        entry.key.clone(),
                        MergedEntry {
                            revision: entry.revision,
                            completion: entry.completion,
                            recorded_at: entry.recorded_at,
                        },
                    );
                    outcome.inserted += 1;
                }
                Some(existing) => {
                    let revision = existing.revision.max(entry.revision);
                    let completion = existing.completion.max(entry.completion);
                    let recorded_at = existing.recorded_at.max(entry.recorded_at);
                    if revision == existing.revision
                        && completion == existing.completion
                        && recorded_at == existing.recorded_at
                    {
                        outcome.unchanged += 1;
                    } else {
                        existing.revision = revision;
                        existing.completion = completion;
                        existing.recorded_at = recorded_at;
                        outcome.updated += 1;
                    }
                }
            }
        }
        outcome
    }

    /// Number of keys at `Completed` or better.
    pub fn completed_count(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.completion >= Completion::Completed)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, revision: u64, completion: Completion, at: i64) -> Entry {
        Entry {
            key: key.into(),
            revision,
            completion,
            recorded_at: Timestamp::now().plus_seconds(at),
        }
    }

    fn checkpoint(entries: Vec<Entry>) -> Checkpoint {
        Checkpoint {
            format_version: CHECKPOINT_FORMAT_VERSION,
            device: "device-a".into(),
            entries,
        }
    }

    #[test]
    fn merge_is_idempotent() {
        let cp = checkpoint(vec![entry(
            "learn/ownership/move",
            3,
            Completion::Completed,
            0,
        )]);
        let mut state = ProgressState::default();

        let first = state.merge(&cp);
        assert_eq!(first.inserted, 1);
        assert!(first.changed());
        let snapshot = state.clone();

        let second = state.merge(&cp);
        assert_eq!(second.unchanged, 1);
        assert!(!second.changed(), "replaying a checkpoint must be a no-op");
        assert_eq!(state, snapshot);
    }

    #[test]
    fn merge_is_commutative_across_devices() {
        let a = checkpoint(vec![entry("k", 5, Completion::Started, 0)]);
        let mut b = checkpoint(vec![entry("k", 2, Completion::Mastered, 10)]);
        b.device = "device-b".into();

        let mut left = ProgressState::default();
        left.merge(&a);
        left.merge(&b);

        let mut right = ProgressState::default();
        right.merge(&b);
        right.merge(&a);

        assert_eq!(left, right, "checkpoint order must not matter");
        let merged = &left.entries["k"];
        assert_eq!(merged.revision, 5);
        assert_eq!(merged.completion, Completion::Mastered);
    }

    #[test]
    fn a_stale_device_cannot_demote_progress() {
        let mut state = ProgressState::default();
        state.merge(&checkpoint(vec![entry("k", 9, Completion::Mastered, 100)]));

        let mut stale = checkpoint(vec![entry("k", 1, Completion::NotStarted, -1_000)]);
        stale.device = "old-laptop".into();
        let outcome = state.merge(&stale);

        assert_eq!(outcome.unchanged, 1);
        assert_eq!(state.entries["k"].completion, Completion::Mastered);
        assert_eq!(state.entries["k"].revision, 9);
    }

    #[test]
    fn completion_is_a_lattice_ordered_by_depth() {
        assert!(Completion::NotStarted < Completion::Started);
        assert!(Completion::Started < Completion::Completed);
        assert!(Completion::Completed < Completion::Mastered);
    }

    #[test]
    fn batch_size_is_bounded_to_keep_the_api_light() {
        let entries: Vec<_> = (0..=MAX_ENTRIES_PER_BATCH)
            .map(|i| entry(&format!("k{i}"), 1, Completion::Started, 0))
            .collect();
        let err = checkpoint(entries).validate().unwrap_err();
        assert_eq!(err.code(), "invalid_request");
    }

    #[test]
    fn rejects_wrong_format_version_and_bad_keys() {
        let mut cp = checkpoint(vec![entry("k", 1, Completion::Started, 0)]);
        cp.format_version = 999;
        assert!(cp.validate().is_err());

        assert!(
            checkpoint(vec![entry("Bad Key", 1, Completion::Started, 0)])
                .validate()
                .is_err()
        );
        assert!(checkpoint(vec![]).validate().is_err());
    }

    #[test]
    fn accepts_a_well_formed_batch() {
        let cp = checkpoint(vec![
            entry(
                "learn/ownership/move-semantics",
                1,
                Completion::Completed,
                0,
            ),
            entry("quiz/ownership.predict-1", 2, Completion::Mastered, 0),
        ]);
        assert!(cp.validate().is_ok());
    }

    #[test]
    fn counts_completed_units() {
        let mut state = ProgressState::default();
        state.merge(&checkpoint(vec![
            entry("a", 1, Completion::Started, 0),
            entry("b", 1, Completion::Completed, 0),
            entry("c", 1, Completion::Mastered, 0),
        ]));
        assert_eq!(state.completed_count(), 2);
    }

    #[test]
    fn checkpoint_round_trips_through_json() {
        let cp = checkpoint(vec![entry("learn/x", 1, Completion::Started, 0)]);
        let json = serde_json::to_string(&cp).unwrap();
        assert_eq!(serde_json::from_str::<Checkpoint>(&json).unwrap(), cp);
    }
}
