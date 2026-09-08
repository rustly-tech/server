//! Typed identifiers.
//!
//! Every entity id is a distinct type. `Id<Submission>` cannot be passed where
//! `Id<Trial>` is expected, which removes an entire class of argument-order bug
//! from the storage layer without any runtime cost.

use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// A UUIDv7 identifier tagged with the entity type it names.
///
/// UUIDv7 is time-ordered, so ids sort by creation time and index well in
/// Postgres without a separate sequence column.
pub struct Id<T> {
    value: Uuid,
    _tag: PhantomData<fn() -> T>,
}

impl<T> Id<T> {
    /// Mint a new time-ordered identifier.
    pub fn new() -> Self {
        Self {
            value: Uuid::now_v7(),
            _tag: PhantomData,
        }
    }

    /// Wrap an existing UUID, e.g. one read back from storage.
    pub const fn from_uuid(value: Uuid) -> Self {
        Self {
            value,
            _tag: PhantomData,
        }
    }

    /// The underlying UUID.
    pub const fn as_uuid(&self) -> Uuid {
        self.value
    }
}

impl<T> Default for Id<T> {
    fn default() -> Self {
        Self::new()
    }
}

// Manual impls: deriving would add a spurious `T: Clone`-style bound on the tag.
impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Id<T> {}
impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl<T> Eq for Id<T> {}
impl<T> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}
impl<T> std::hash::Hash for Id<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}
impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}
impl<T> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}
impl<T> FromStr for Id<T> {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from_uuid(Uuid::parse_str(s)?))
    }
}
impl<T> Serialize for Id<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(s)
    }
}
impl<'de, T> Deserialize<'de> for Id<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::from_uuid(Uuid::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Trial;
    struct Submission;

    #[test]
    fn ids_are_time_ordered() {
        let a: Id<Trial> = Id::new();
        let b: Id<Trial> = Id::new();
        assert!(a <= b, "UUIDv7 ids must be non-decreasing: {a} then {b}");
    }

    #[test]
    fn round_trips_through_string_and_json() {
        let id: Id<Submission> = Id::new();
        assert_eq!(id, id.to_string().parse().unwrap());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(id, serde_json::from_str::<Id<Submission>>(&json).unwrap());
    }

    #[test]
    fn distinct_tags_are_distinct_types() {
        // This is the point of the newtype; assert it stays true by construction.
        let uuid = Uuid::now_v7();
        let trial = Id::<Trial>::from_uuid(uuid);
        let submission = Id::<Submission>::from_uuid(uuid);
        assert_eq!(trial.as_uuid(), submission.as_uuid());
        // `assert_eq!(trial, submission)` does not compile, which is the guarantee.
    }
}
