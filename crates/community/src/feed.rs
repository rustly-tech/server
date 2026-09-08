//! The Recent feed and the Archive.

use rustly_common::{Result, Timestamp};
use rustly_domain::event::{EventId, RECENT_WINDOW_SECONDS};
use rustly_domain::{EventKind, PlatformEvent, TrialMilestone, Username};
use rustly_storage::MetadataStore;

/// Default page size for both feeds.
pub const DEFAULT_LIMIT: usize = 50;

/// Maximum page size a caller may request.
pub const MAX_LIMIT: usize = 200;

/// Read the Recent feed: meaningful events from the last 24 hours, newest first.
pub async fn recent<S: MetadataStore + ?Sized>(
    store: &S,
    limit: usize,
) -> Result<Vec<PlatformEvent>> {
    let since = Timestamp::now().plus_seconds(-RECENT_WINDOW_SECONDS);
    store
        .events(Some(since), None, limit.clamp(1, MAX_LIMIT))
        .await
}

/// Read the Archive: the full meaningful-event history, optionally by kind.
pub async fn archive<S: MetadataStore + ?Sized>(
    store: &S,
    kind: Option<EventKind>,
    limit: usize,
) -> Result<Vec<PlatformEvent>> {
    store.events(None, kind, limit.clamp(1, MAX_LIMIT)).await
}

/// Build the event emitted when a user upgrades their Trial milestone badge.
///
/// One event per milestone crossing, never one per solve. That distinction is
/// what keeps Recent meaningful rather than a stream of "user solved a Trial".
pub fn milestone_event(username: &Username, milestone: TrialMilestone) -> PlatformEvent {
    PlatformEvent {
        id: EventId::new(),
        kind: EventKind::MilestoneReached,
        summary: format!("{username} reached {} Trials solved", milestone.threshold()),
        href: Some(format!("/users/{username}")),
        subject: Some(username.to_string()),
        occurred_at: Timestamp::now(),
    }
}

/// Build the event emitted when a Trial becomes publicly available.
pub fn trial_published_event(slug: &str, title: &str) -> PlatformEvent {
    PlatformEvent {
        id: EventId::new(),
        kind: EventKind::TrialPublished,
        summary: format!("New Trial: {title}"),
        href: Some(format!("/trials/{slug}")),
        subject: None,
        occurred_at: Timestamp::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustly_storage::memory::MemoryStore;

    async fn seeded() -> MemoryStore {
        let store = MemoryStore::new();
        let now = Timestamp::now();
        for (offset, kind, summary) in [
            (-60_i64, EventKind::TrialPublished, "fresh trial"),
            (-3_600, EventKind::LessonPublished, "fresh lesson"),
            (-90_000, EventKind::TrialPublished, "old trial"),
        ] {
            store
                .append_event(&PlatformEvent {
                    id: EventId::new(),
                    kind,
                    summary: summary.into(),
                    href: None,
                    subject: None,
                    occurred_at: now.plus_seconds(offset),
                })
                .await
                .unwrap();
        }
        store
    }

    #[tokio::test]
    async fn recent_shows_only_the_last_day_newest_first() {
        let events = recent(&seeded().await, DEFAULT_LIMIT).await.unwrap();
        let summaries: Vec<_> = events.iter().map(|e| e.summary.as_str()).collect();
        assert_eq!(summaries, ["fresh trial", "fresh lesson"]);
    }

    #[tokio::test]
    async fn archive_shows_everything_and_filters_by_kind() {
        let store = seeded().await;
        assert_eq!(archive(&store, None, DEFAULT_LIMIT).await.unwrap().len(), 3);

        let trials = archive(&store, Some(EventKind::TrialPublished), DEFAULT_LIMIT)
            .await
            .unwrap();
        assert_eq!(trials.len(), 2);
        assert!(trials.iter().all(|e| e.kind == EventKind::TrialPublished));
    }

    #[tokio::test]
    async fn page_size_is_clamped() {
        let store = seeded().await;
        assert_eq!(recent(&store, 1).await.unwrap().len(), 1);
        // A caller asking for a million rows gets MAX_LIMIT, not an outage.
        assert!(archive(&store, None, usize::MAX).await.unwrap().len() <= MAX_LIMIT);
        assert!(
            !archive(&store, None, 0).await.unwrap().is_empty(),
            "0 must not mean 0 rows"
        );
    }

    #[test]
    fn milestone_events_are_per_threshold_not_per_solve() {
        let username = Username::parse("ferris").unwrap();
        let event = milestone_event(&username, TrialMilestone::Fifty);
        assert_eq!(event.kind, EventKind::MilestoneReached);
        assert!(event.summary.contains("50"));
        assert_eq!(event.href.as_deref(), Some("/users/ferris"));
        assert_eq!(event.subject.as_deref(), Some("ferris"));
    }

    #[test]
    fn trial_publication_links_to_the_trial() {
        let event = trial_published_event("ownership-move-or-borrow", "Move or Borrow?");
        assert_eq!(event.kind, EventKind::TrialPublished);
        assert_eq!(
            event.href.as_deref(),
            Some("/trials/ownership-move-or-borrow")
        );
    }
}
