# Rustly control-plane API v1

Base path `/api/v1`. Version reported by `GET /api/version`. Content type is
`application/json` throughout, except the SSE stream.

## What this API is not

It is **not** the read path for lessons, cheatsheets, or Trial statements. Those
are static, content-addressed, and served from a CDN or a local cache; a browser
must be able to render `/learn/*`, `/cheatsheets/*`, and `/trials/*` with this
API unreachable (invariant A).

It is **not** a blob transport. The largest thing it accepts is 64 KiB, enforced
by `RequestBodyLimitLayer`. Source code, test data, and result manifests travel
through the data plane; the API carries CIDs.

It **never** executes user code (invariant G).

## Authentication

Bearer tokens: `Authorization: Bearer v1.<payload>.<mac>`, HMAC-SHA256 over the
version prefix and payload. Two disjoint principal families:

| Principal | Scopes |
| --- | --- |
| User | `read_self`, `write_progress`, `submit` |
| Judge worker | `judge_work` |

A user token can never lease a job; a worker token can never read an account.
A worker's trust class comes from its **token**, never from a request body.

Anonymous callers get `401`. Authenticated callers missing a scope get `403`,
because signing in again will not help them.

## Errors

Every non-2xx response has the same body:

```json
{ "code": "not_found", "message": "trial not found: ownership", "request_id": "018f..." }
```

`code` values are stable: `not_found`, `invalid_request`, `conflict`,
`unauthenticated`, `forbidden`, `dependency_unavailable`, `internal`.

Dependency and internal failures are logged in full and summarised to the
caller. A connection string never reaches a client.

## Correlation

Send `x-request-id` to join a trace. Values are accepted only if they are 1-64
characters of `[A-Za-z0-9._-]`; anything else is replaced with a fresh UUID
rather than echoed, because the value ends up in logs and response bodies.

## Endpoints

### `GET /health`

Liveness. Touches no dependency, so a database outage does not cause an
orchestrator to kill an otherwise healthy process.

### `GET /ready`

Readiness. Pings the metadata store. `503` with `"status": "degraded"` when it
is unreachable.

### `GET /api/version`

```json
{
  "api_version": 1,
  "broker_protocol_version": 1,
  "build": "3f2a1c9",
  "ranking_model": "provisional-v0",
  "ranking_provisional": true
}
```

`ranking_provisional` exists so the UI can label the rank number honestly rather
than implying a validated rating.

### `GET /api/v1/users/{username}`

The minimal public profile. Exactly seven fields:

```json
{
  "profile": {
    "username": "ferris",
    "clan": "RUST",
    "global_rank": 1,
    "rank": 8.0,
    "level": 2,
    "trials_solved": 1,
    "highest_trial_milestone": null
  }
}
```

Deliberately absent, and blocked by a test: country or regional ranking, public
performance history, contribution heatmaps, course badges, stacked milestone
badges, tiered (gold/silver/diamond) presentation.

`global_rank` is `null` until the user has scored anything.
`highest_trial_milestone` is a single value that upgrades in place
(10 -> 50 -> 100 -> 250 -> 500 -> 1000), never a list.

### `GET /api/v1/me`

Requires `read_self`. Username, rank, level, trials solved, supporter flag.
Supporter is cosmetic only and never affects ranking, judging, or priority.

### `GET /api/v1/trials`

Query: `difficulty`, `topic`, `status`. Returns metadata plus `content_cid`.
Draft Trials are never listed.

### `GET /api/v1/trials/{slug}`

One Trial's metadata. A draft Trial returns `404`, identical to a missing one,
so unpublished slugs cannot be probed for.

### `POST /api/v1/trials/{slug}/reveal`

Requires `submit`. Records that the user revealed published solutions. This
forfeits first-solve ranking credit for that Trial and nothing else: the solve
still counts toward the milestone badge and still awards experience.

### `GET /api/v1/progress`

Requires `read_self`. Merged local-first progress.

### `POST /api/v1/progress/checkpoints`

Requires `write_progress`. Body:

```json
{
  "format_version": 1,
  "device": "browser-a",
  "entries": [
    { "key": "learn/ownership/move-semantics", "revision": 3,
      "completion": "completed", "recorded_at": "2026-01-01T00:00:00Z" }
  ]
}
```

At most 256 entries per batch. The merge is idempotent, commutative, and
monotonic: replaying changes nothing, order does not matter, and a stale device
cannot demote progress. The response reports `inserted` / `updated` /
`unchanged`, so a retry is visibly a no-op.

The API never receives keystrokes, editor heartbeats, lesson views, individual
quiz clicks, or local Runs (invariant C).

### `POST /api/v1/submissions`

Requires `submit`. Body carries a **CID**, not source:

```json
{ "trial": "ownership-move-or-borrow", "source_cid": "b3:...", "idempotency_key": "..." }
```

`202` on creation, `200` with `"idempotent_replay": true` when the idempotency
key has been seen. The `job_id` is minted once and is immutable.

### `GET /api/v1/submissions/{id}`

Another user's submission returns `404`, not `403`, so ids cannot be probed.

### `GET /api/v1/submissions/{id}/events`

Server-Sent Events. Emits the current state immediately, then each transition,
and closes after the terminal event.

SSE rather than a WebSocket: this is a short-lived, server-to-client, text-only
stream. A WebSocket would add a second transport and a second set of proxy
problems for no capability we need. WebSockets are reserved for genuinely
bidirectional realtime features.

A subscriber that falls behind is disconnected rather than buffered without
bound; it re-reads authoritative state with a plain `GET`.

### Judge broker

`POST /api/v1/judge/leases` - a worker claims up to `capacity` jobs.
`POST /api/v1/judge/jobs/{job_id}/heartbeat` - coarse progress.
`POST /api/v1/judge/jobs/{job_id}/result` - final verdict; idempotent.

Pull-based on purpose: workers may be behind NAT, may be volunteer capacity, and
may disappear mid-job.

A lease carries identifiers and limits, never payloads.
`may_receive_hidden_tests` is `true` only for a `trusted` worker. A worker that
declares a higher trust class than its credential grants is **refused**, not
silently downgraded, so the attempt is visible in logs.

### `GET /api/v1/recent`, `GET /api/v1/archive`

Recent is a 24-hour window of meaningful events. Archive is the same log without
the window, filterable by `kind`. Event kinds are a closed, curated set: no
"user viewed a lesson", no "user ran code".

### `GET /api/v1/clans/{tag}`

Clan identity and member list. No currency, no levels, no territory, no roles.

## Verdicts

`AC` `CE` `WA` `TLE` `MLE` `OLE` `RTE` `JE` `IE` `SE`

`JE` and `IE` are **system faults**. They are recorded for operators, are
retryable, and never mark a Trial as attempted. An infrastructure failure is
never reported to a user as if they had made a mistake (invariant H).
