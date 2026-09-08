//! Platform events: the source of the Recent feed and the Archive.
//!
//! Recent is a 24-hour window of *meaningful* events. It is explicitly not a
//! user activity feed: opening a lesson, running code locally, or answering a
//! quiz never produces an event. Only the kinds enumerated in [`EventKind`] do.

use rustly_common::{Id, Timestamp};
use serde::{Deserialize, Serialize};

/// Type tag for an event id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventTag;
/// An event identifier.
pub type EventId = Id<EventTag>;

/// The closed set of event kinds that may appear in Recent or Archive.
///
/// Adding a variant is a product decision. The test below pins the set so that a
/// social-feed-shaped event kind cannot be added without someone noticing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A new Trial reached Verified or Official.
    TrialPublished,
    /// A Trial was materially revised (new content version).
    TrialRevised,
    /// A new lesson or learning path was published.
    LessonPublished,
    /// A cheatsheet was published or substantially revised.
    CheatsheetPublished,
    /// A curated OSS example was added or repinned to a new commit.
    OssExampleAdded,
    /// An event (official, topic, OSS, or community) opened.
    EventStarted,
    /// An event closed.
    EventEnded,
    /// A user reached a Trial milestone. One event per milestone, not per solve.
    MilestoneReached,
    /// A community contribution was accepted.
    ContributionAccepted,
}

impl EventKind {
    /// Every event kind.
    pub const ALL: [EventKind; 9] = [
        Self::TrialPublished,
        Self::TrialRevised,
        Self::LessonPublished,
        Self::CheatsheetPublished,
        Self::OssExampleAdded,
        Self::EventStarted,
        Self::EventEnded,
        Self::MilestoneReached,
        Self::ContributionAccepted,
    ];
}

/// A meaningful platform event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlatformEvent {
    /// Stable identifier.
    pub id: EventId,
    /// What happened.
    pub kind: EventKind,
    /// Short human summary, already rendered.
    pub summary: String,
    /// Relative link into the site, if the event points somewhere.
    pub href: Option<String>,
    /// Subject username, when the event is about a person.
    pub subject: Option<String>,
    /// When it happened.
    pub occurred_at: Timestamp,
}

/// The Recent window, in seconds.
pub const RECENT_WINDOW_SECONDS: i64 = 24 * 60 * 60;

impl PlatformEvent {
    /// Whether this event belongs in the Recent feed relative to `now`.
    pub fn is_recent(&self, now: Timestamp) -> bool {
        self.occurred_at
            .within_seconds_before(now, RECENT_WINDOW_SECONDS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: EventKind, occurred_at: Timestamp) -> PlatformEvent {
        PlatformEvent {
            id: EventId::new(),
            kind,
            summary: "something meaningful".into(),
            href: None,
            subject: None,
            occurred_at,
        }
    }

    #[test]
    fn recent_window_is_exactly_twenty_four_hours() {
        assert_eq!(RECENT_WINDOW_SECONDS, 86_400);
        let now = Timestamp::now();
        assert!(event(EventKind::TrialPublished, now.plus_seconds(-86_399)).is_recent(now));
        assert!(event(EventKind::TrialPublished, now.plus_seconds(-86_400)).is_recent(now));
        assert!(!event(EventKind::TrialPublished, now.plus_seconds(-86_401)).is_recent(now));
    }

    #[test]
    fn event_kinds_are_a_closed_curated_set() {
        // If this fails, someone added an event kind. Confirm it is genuinely
        // meaningful and not activity-feed spam before updating the count.
        assert_eq!(EventKind::ALL.len(), 9);
        let names: Vec<String> = EventKind::ALL
            .iter()
            .map(|k| serde_json::to_string(k).unwrap())
            .collect();
        assert!(
            !names
                .iter()
                .any(|n| n.contains("view") || n.contains("run") || n.contains("login")),
            "Recent must not become an activity feed: {names:?}"
        );
    }
}
