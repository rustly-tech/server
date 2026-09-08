//! Clans.
//!
//! Deliberately restrained: name, tag, identity, membership, discovery. There is
//! no clan currency, no clan levels, no skill tree, no territory, no MMO roles,
//! and no way to buy capacity. Adding any of those is a product change, not a
//! feature increment.

use rustly_common::{Error, Id, Result, Timestamp};
use serde::{Deserialize, Serialize};

use crate::user::UserId;

/// Type tag for a clan id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClanTagType;
/// A clan identifier.
pub type ClanId = Id<ClanTagType>;

/// A short clan tag shown next to usernames, e.g. `RUST`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ClanTag(String);

impl ClanTag {
    /// Minimum length.
    pub const MIN_LEN: usize = 2;
    /// Maximum length. Short by design: it renders inline next to a username.
    pub const MAX_LEN: usize = 6;

    /// Validate and construct: 2-6 uppercase ASCII alphanumerics.
    pub fn parse(raw: &str) -> Result<Self> {
        let len = raw.chars().count();
        if !(Self::MIN_LEN..=Self::MAX_LEN).contains(&len) {
            return Err(Error::invalid(
                "clan_tag",
                format!("must be {}-{} characters", Self::MIN_LEN, Self::MAX_LEN),
            ));
        }
        if !raw
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            return Err(Error::invalid("clan_tag", "may contain only A-Z and 0-9"));
        }
        Ok(Self(raw.to_owned()))
    }

    /// The tag as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ClanTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ClanTag {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        ClanTag::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// A clan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clan {
    /// Stable identifier.
    pub id: ClanId,
    /// Short tag.
    pub tag: ClanTag,
    /// Display name.
    pub name: String,
    /// One-line description.
    pub blurb: Option<String>,
    /// Owning user.
    pub owner: UserId,
    /// Creation time.
    pub created_at: Timestamp,
}

/// Membership of a user in a clan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClanMembership {
    /// The clan.
    pub clan_id: ClanId,
    /// The member.
    pub user_id: UserId,
    /// When they joined.
    pub joined_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_short_uppercase_tags() {
        for t in ["AB", "RUST", "R2D2", "ABCDEF"] {
            assert!(ClanTag::parse(t).is_ok(), "should accept {t}");
        }
    }

    #[test]
    fn rejects_lowercase_symbols_and_wrong_lengths() {
        for t in ["a", "rust", "TOOLONGX", "R-2", "RU ST", ""] {
            assert!(ClanTag::parse(t).is_err(), "should reject {t:?}");
        }
    }

    #[test]
    fn clan_carries_no_progression_or_economy_fields() {
        let clan = Clan {
            id: ClanId::new(),
            tag: ClanTag::parse("RUST").unwrap(),
            name: "Rustaceans".into(),
            blurb: None,
            owner: UserId::new(),
            created_at: Timestamp::now(),
        };
        let value = serde_json::to_value(&clan).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["blurb", "created_at", "id", "name", "owner", "tag"]
        );
    }
}
