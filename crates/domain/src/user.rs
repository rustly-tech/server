//! Users, and the deliberately minimal public profile.

use rustly_common::{Error, Id, Result, Timestamp};
use serde::{Deserialize, Serialize};

use crate::clan::ClanTag;
use crate::milestone::TrialMilestone;

/// Type tag for [`rustly_common::Id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserTag;

/// A user identifier.
pub type UserId = Id<UserTag>;

/// A validated username.
///
/// Rules: 3-32 characters, ASCII alphanumeric plus `-` and `_`, must start with
/// an alphanumeric. Compared case-insensitively so `Ferris` and `ferris` cannot
/// both exist and impersonate each other.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Username(String);

impl Username {
    /// Minimum length in characters.
    pub const MIN_LEN: usize = 3;
    /// Maximum length in characters.
    pub const MAX_LEN: usize = 32;

    /// Validate and construct.
    pub fn parse(raw: &str) -> Result<Self> {
        let len = raw.chars().count();
        if len < Self::MIN_LEN {
            return Err(Error::invalid(
                "username",
                format!("must be at least {} characters", Self::MIN_LEN),
            ));
        }
        if len > Self::MAX_LEN {
            return Err(Error::invalid(
                "username",
                format!("must be at most {} characters", Self::MAX_LEN),
            ));
        }
        if !raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(Error::invalid(
                "username",
                "may contain only ASCII letters, digits, '-' and '_'",
            ));
        }
        if !raw
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        {
            return Err(Error::invalid(
                "username",
                "must start with a letter or digit",
            ));
        }
        Ok(Self(raw.to_owned()))
    }

    /// The username as displayed, preserving the case the user chose.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The lookup key: lowercase, so uniqueness is case-insensitive.
    pub fn lookup_key(&self) -> String {
        self.0.to_ascii_lowercase()
    }
}

impl std::fmt::Display for Username {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Username {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Username::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// A user's provisional rank score.
///
/// The number itself carries no meaning outside the ranking model that produced
/// it, which is why the model id travels with it. See `rustly-ranking`.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RankScore(pub f64);

impl RankScore {
    /// Zero score, for a user who has solved nothing.
    pub const ZERO: RankScore = RankScore(0.0);
}

/// A user's level.
///
/// Level is a monotone function of accumulated experience. It is a progress
/// signal, not a skill claim, and it never decreases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Level(pub u32);

/// The stored user record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    /// Stable identifier.
    pub id: UserId,
    /// Display username.
    pub username: Username,
    /// Clan tag, if the user is in a clan.
    pub clan: Option<ClanTag>,
    /// Provisional rank score.
    pub rank_score: RankScore,
    /// Current level.
    pub level: Level,
    /// Count of distinct Trials solved.
    pub trials_solved: u32,
    /// Whether the user has an active supporter entitlement (cosmetic only).
    pub supporter: bool,
    /// Account creation time.
    pub created_at: Timestamp,
}

/// The public profile.
///
/// This shape is a product decision, not an implementation detail. It contains
/// exactly seven fields and deliberately omits country and regional ranking,
/// public performance-history graphs, contribution heatmaps, course badges,
/// stacked milestone badges, and tiered (gold/silver/diamond) presentation.
/// Adding any of those is a product change requiring its own review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicProfile {
    /// Display username.
    pub username: Username,
    /// Clan tag, if any.
    pub clan: Option<ClanTag>,
    /// Position in the global ordering, 1-based. `None` while unranked.
    pub global_rank: Option<u64>,
    /// Provisional rank score.
    pub rank: RankScore,
    /// Level.
    pub level: Level,
    /// Distinct Trials solved.
    pub trials_solved: u32,
    /// Highest achieved Trial milestone badge, upgraded in place. `None` below
    /// the first threshold.
    pub highest_trial_milestone: Option<TrialMilestone>,
}

impl User {
    /// Project the public profile for this user.
    pub fn public_profile(&self, global_rank: Option<u64>) -> PublicProfile {
        PublicProfile {
            username: self.username.clone(),
            clan: self.clan.clone(),
            global_rank,
            rank: self.rank_score,
            level: self.level,
            trials_solved: self.trials_solved,
            highest_trial_milestone: TrialMilestone::highest_for(self.trials_solved),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(trials_solved: u32) -> User {
        User {
            id: UserId::new(),
            username: Username::parse("ferris").unwrap(),
            clan: None,
            rank_score: RankScore(120.0),
            level: Level(4),
            trials_solved,
            supporter: false,
            created_at: Timestamp::now(),
        }
    }

    #[test]
    fn accepts_reasonable_usernames() {
        for name in [
            "abc",
            "ferris",
            "rust-lang",
            "user_1",
            "a1",
            "A".repeat(32).as_str(),
        ] {
            if name.len() >= 3 {
                assert!(Username::parse(name).is_ok(), "should accept {name}");
            }
        }
    }

    #[test]
    fn rejects_bad_usernames() {
        for name in [
            "ab",
            "-lead",
            "_lead",
            "has space",
            "emoji🦀",
            &"a".repeat(33),
        ] {
            assert!(Username::parse(name).is_err(), "should reject {name:?}");
        }
    }

    #[test]
    fn username_lookup_is_case_insensitive_but_display_is_preserved() {
        let a = Username::parse("Ferris").unwrap();
        let b = Username::parse("ferris").unwrap();
        assert_ne!(a, b, "display forms differ");
        assert_eq!(a.lookup_key(), b.lookup_key(), "lookup keys must collide");
        assert_eq!(a.as_str(), "Ferris");
    }

    #[test]
    fn public_profile_exposes_only_the_seven_permitted_fields() {
        let value = serde_json::to_value(user(57).public_profile(Some(42))).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "clan",
                "global_rank",
                "highest_trial_milestone",
                "level",
                "rank",
                "trials_solved",
                "username",
            ]
        );
    }

    #[test]
    fn public_profile_shows_only_the_highest_milestone() {
        let profile = user(57).public_profile(None);
        assert_eq!(profile.highest_trial_milestone, Some(TrialMilestone::Fifty));
        // Not a list: the badge upgrades in place, it does not stack.
        let json = serde_json::to_value(&profile).unwrap();
        assert!(json["highest_trial_milestone"].is_string());
    }

    #[test]
    fn username_deserialisation_validates() {
        assert!(serde_json::from_str::<Username>("\"ferris\"").is_ok());
        assert!(serde_json::from_str::<Username>("\"a b\"").is_err());
    }
}
