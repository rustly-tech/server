//! Recent, Archive, and the clan directory.

use axum::extract::{Path, Query, State};
use axum::Json;
use rustly_domain::{ClanTag, EventKind};
use rustly_protocol::api::{EventEntry, EventListResponse};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::extract::RequestId;
use crate::state::AppState;

/// Query parameters for the event feeds.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FeedQuery {
    /// Filter by event kind. Archive only.
    #[serde(default)]
    pub kind: Option<EventKind>,
    /// Page size.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `GET /api/v1/recent` - meaningful events from the last 24 hours.
pub async fn recent(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Query(query): Query<FeedQuery>,
) -> Result<Json<EventListResponse>, ApiError> {
    let events = rustly_community::feed::recent(
        state.store.as_ref(),
        query.limit.unwrap_or(rustly_community::feed::DEFAULT_LIMIT),
    )
    .await
    .map_err(|e| ApiError::new(e, request_id))?;
    Ok(Json(EventListResponse {
        events: events.into_iter().map(entry).collect(),
    }))
}

/// `GET /api/v1/archive` - the full meaningful-event history.
pub async fn archive(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Query(query): Query<FeedQuery>,
) -> Result<Json<EventListResponse>, ApiError> {
    let events = rustly_community::feed::archive(
        state.store.as_ref(),
        query.kind,
        query.limit.unwrap_or(rustly_community::feed::DEFAULT_LIMIT),
    )
    .await
    .map_err(|e| ApiError::new(e, request_id))?;
    Ok(Json(EventListResponse {
        events: events.into_iter().map(entry).collect(),
    }))
}

/// A clan and its members.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClanResponse {
    /// Short tag.
    pub tag: String,
    /// Display name.
    pub name: String,
    /// One-line description.
    pub blurb: Option<String>,
    /// Member usernames.
    pub members: Vec<String>,
}

/// `GET /api/v1/clans/{tag}`.
pub async fn clan(
    State(state): State<AppState>,
    RequestId(request_id): RequestId,
    Path(tag): Path<String>,
) -> Result<Json<ClanResponse>, ApiError> {
    let tag = ClanTag::parse(&tag).map_err(|e| ApiError::new(e, request_id.clone()))?;
    let clan = state
        .store
        .clan_by_tag(&tag)
        .await
        .map_err(|e| ApiError::new(e, request_id.clone()))?;
    let members = state
        .store
        .clan_members(&tag)
        .await
        .map_err(|e| ApiError::new(e, request_id))?;

    Ok(Json(ClanResponse {
        tag: clan.tag.to_string(),
        name: clan.name,
        blurb: clan.blurb,
        members: members.iter().map(|u| u.to_string()).collect(),
    }))
}

fn entry(event: rustly_domain::PlatformEvent) -> EventEntry {
    EventEntry {
        kind: event.kind,
        summary: event.summary,
        href: event.href,
        subject: event.subject,
        occurred_at: event.occurred_at,
    }
}
