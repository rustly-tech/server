//! Seed data for the Ownership vertical slice.
//!
//! Used by local development and by the integration tests. In production the
//! Trial catalogue is published from the `content` repository; this seed exists
//! so that `cargo run` gives a working slice with no external dependency.

use std::sync::Arc;

use rustly_common::{Result, Timestamp};
use rustly_domain::trial::TrialId;
use rustly_domain::{Difficulty, Trial, TrialLifecycle, TrialSlug, TrialTopic, Username};
use rustly_storage::MetadataStore;

/// Slug of the Trial that the first vertical slice ships.
pub const SLICE_TRIAL_SLUG: &str = "ownership-move-or-borrow";

/// Insert the Ownership slice Trial and a demo user.
///
/// Returns the demo user's id so a caller can mint a token for it.
pub async fn ownership_slice(
    store: &Arc<dyn MetadataStore>,
) -> Result<rustly_domain::user::UserId> {
    let trial = Trial {
        id: TrialId::new(),
        slug: TrialSlug::parse(SLICE_TRIAL_SLUG)?,
        title: "Move or Borrow?".into(),
        difficulty: Difficulty::Easy,
        topics: vec![TrialTopic::Ownership, TrialTopic::Borrowing],
        lifecycle: TrialLifecycle::Official,
        version: 2,
        // CID of evaluation/trials/ownership-move-or-borrow/v2/package.json.
        // The package itself stays private and is fetched only by trusted workers.
        content_cid: "b3:25d53043ef9403604ed11f508820065f875446d4eafdba694c2d00826025621c".into(),
        published_at: Timestamp::now(),
    };
    store.put_trial(&trial).await?;

    store
        .append_event(&rustly_community::feed::trial_published_event(
            trial.slug.as_str(),
            &trial.title,
        ))
        .await?;

    let user = store.create_user(&Username::parse("ferris")?).await?;
    Ok(user.id)
}
