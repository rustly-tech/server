//! Trials: the judged practice unit.

use rustly_common::{Error, Id, Result, Timestamp};
use serde::{Deserialize, Serialize};

/// Type tag for [`rustly_common::Id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrialTag;

/// A Trial identifier.
pub type TrialId = Id<TrialTag>;

/// A URL-safe Trial slug, e.g. `ownership-move-or-borrow`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct TrialSlug(String);

impl TrialSlug {
    /// Validate and construct: 1-96 lowercase ASCII alphanumerics and hyphens,
    /// no leading, trailing, or doubled hyphen.
    pub fn parse(raw: &str) -> Result<Self> {
        if raw.is_empty() || raw.len() > 96 {
            return Err(Error::invalid("slug", "must be 1-96 characters"));
        }
        if !raw
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(Error::invalid("slug", "may contain only a-z, 0-9 and '-'"));
        }
        if raw.starts_with('-') || raw.ends_with('-') || raw.contains("--") {
            return Err(Error::invalid(
                "slug",
                "hyphens must separate non-empty segments",
            ));
        }
        Ok(Self(raw.to_owned()))
    }

    /// The slug as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TrialSlug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TrialSlug {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        TrialSlug::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Difficulty tier. Ordering is meaningful and used by the ranking model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    /// First contact with a concept.
    Intro,
    /// Routine application of one concept.
    Easy,
    /// Combines concepts or needs a non-obvious step.
    Medium,
    /// Requires real design judgement.
    Hard,
    /// Deep or adversarial.
    Expert,
}

impl Difficulty {
    /// All tiers, ascending.
    pub const ALL: [Difficulty; 5] = [
        Self::Intro,
        Self::Easy,
        Self::Medium,
        Self::Hard,
        Self::Expert,
    ];
}

/// A topic tag, used for filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialTopic {
    /// Ownership, moves, `Copy`.
    Ownership,
    /// Shared and unique borrows.
    Borrowing,
    /// Lifetimes and variance.
    Lifetimes,
    /// Traits and generics.
    Traits,
    /// Enums, pattern matching, `Option`/`Result`.
    Enums,
    /// Collections and iterators.
    Collections,
    /// Error handling.
    Errors,
    /// Concurrency and `Send`/`Sync`.
    Concurrency,
    /// Unsafe and FFI.
    Unsafe,
    /// Modules, crates, Cargo.
    Tooling,
}

/// Editorial lifecycle of a community-contributed Trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialLifecycle {
    /// Author is still working; not listed.
    Draft,
    /// Listed with a beta marker; solves do not yet count toward ranking.
    Beta,
    /// Reviewed and accepted; solves count.
    Verified,
    /// Curated by maintainers as canonical for a concept.
    Official,
}

impl TrialLifecycle {
    /// Whether a Trial in this state is publicly listed.
    pub const fn is_listed(self) -> bool {
        !matches!(self, Self::Draft)
    }

    /// Whether solving a Trial in this state contributes to rank.
    ///
    /// Only reviewed content moves the ranking, so an author cannot inflate
    /// their own score by publishing trivial Trials.
    pub const fn counts_for_ranking(self) -> bool {
        matches!(self, Self::Verified | Self::Official)
    }
}

/// Per-user status of a Trial. Exactly three states are shown in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialStatus {
    /// Never submitted.
    Unsolved,
    /// Submitted at least once, never accepted.
    Attempted,
    /// Accepted at least once.
    Solved,
}

/// Trial metadata.
///
/// This is metadata only. Statements, starter code, public tests, and reference
/// solutions are immutable content addressed by [`Trial::content_cid`] and served
/// from the data plane, never from the control-plane database. Hidden tests are
/// never represented here at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trial {
    /// Stable identifier.
    pub id: TrialId,
    /// URL slug.
    pub slug: TrialSlug,
    /// Human title.
    pub title: String,
    /// Difficulty tier.
    pub difficulty: Difficulty,
    /// Topics, for filtering.
    pub topics: Vec<TrialTopic>,
    /// Editorial state.
    pub lifecycle: TrialLifecycle,
    /// Content version. A verdict is only meaningful against a specific version.
    pub version: u32,
    /// BLAKE3 CID of the immutable Trial package in the data plane.
    pub content_cid: String,
    /// Publication time.
    pub published_at: Timestamp,
}

impl Trial {
    /// Whether this Trial may be listed publicly.
    pub const fn is_listed(&self) -> bool {
        self.lifecycle.is_listed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_well_formed_slugs() {
        for s in ["a", "ownership-move", "trial-42", "x-y-z"] {
            assert!(TrialSlug::parse(s).is_ok(), "should accept {s}");
        }
    }

    #[test]
    fn rejects_malformed_slugs() {
        for s in [
            "",
            "-lead",
            "trail-",
            "double--hyphen",
            "Upper",
            "has space",
            "under_score",
        ] {
            assert!(TrialSlug::parse(s).is_err(), "should reject {s:?}");
        }
    }

    #[test]
    fn draft_trials_are_not_listed() {
        assert!(!TrialLifecycle::Draft.is_listed());
        assert!(TrialLifecycle::Beta.is_listed());
        assert!(TrialLifecycle::Verified.is_listed());
        assert!(TrialLifecycle::Official.is_listed());
    }

    #[test]
    fn only_reviewed_trials_move_the_ranking() {
        assert!(!TrialLifecycle::Draft.counts_for_ranking());
        assert!(!TrialLifecycle::Beta.counts_for_ranking());
        assert!(TrialLifecycle::Verified.counts_for_ranking());
        assert!(TrialLifecycle::Official.counts_for_ranking());
    }

    #[test]
    fn difficulty_orders_from_intro_to_expert() {
        assert!(Difficulty::Intro < Difficulty::Easy);
        assert!(Difficulty::Hard < Difficulty::Expert);
        let mut all = Difficulty::ALL;
        all.sort();
        assert_eq!(all, Difficulty::ALL);
    }

    #[test]
    fn wire_names_are_snake_case_and_stable() {
        assert_eq!(
            serde_json::to_string(&Difficulty::Expert).unwrap(),
            "\"expert\""
        );
        assert_eq!(
            serde_json::to_string(&TrialStatus::Solved).unwrap(),
            "\"solved\""
        );
        assert_eq!(
            serde_json::to_string(&TrialLifecycle::Official).unwrap(),
            "\"official\""
        );
        assert_eq!(
            serde_json::to_string(&TrialTopic::Ownership).unwrap(),
            "\"ownership\""
        );
    }
}
