-- Rustly control plane, initial schema.
--
-- Scope rule (architectural invariant D): this database holds small, mutable,
-- authoritative state only. Source code, test data, compiled artifacts, Git
-- packfiles, and result manifests are content-addressed and live in the data
-- plane. Columns here carry CIDs, never bytes.
--
-- Portability rule: standard PostgreSQL only. Neon-compatible, but nothing
-- depends on Neon-specific behaviour, so a plain `postgres:17` container and a
-- managed Neon branch are interchangeable.

CREATE TABLE clans (
    id          UUID PRIMARY KEY,
    tag         TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    blurb       TEXT,
    owner_id    UUID NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT clans_tag_shape CHECK (tag ~ '^[A-Z0-9]{2,6}$')
);

CREATE TABLE users (
    id              UUID PRIMARY KEY,
    username        TEXT NOT NULL,
    -- Lowercased username. Uniqueness is case-insensitive so two accounts
    -- cannot differ only in case and impersonate each other.
    username_key    TEXT NOT NULL UNIQUE,
    clan_id         UUID REFERENCES clans (id) ON DELETE SET NULL,
    rank_score      DOUBLE PRECISION NOT NULL DEFAULT 0,
    -- Id of the ranking model that produced rank_score. The model is
    -- provisional and replaceable; storing its id makes a recompute auditable.
    ranking_model   TEXT NOT NULL DEFAULT 'provisional-v0',
    level           INTEGER NOT NULL DEFAULT 1,
    experience      BIGINT NOT NULL DEFAULT 0,
    trials_solved   INTEGER NOT NULL DEFAULT 0,
    supporter       BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT users_counts_non_negative CHECK (trials_solved >= 0 AND experience >= 0 AND level >= 1)
);

-- Global Rank is an ordering over rank_score, computed at read time.
CREATE INDEX users_rank_score_idx ON users (rank_score DESC);

CREATE TABLE clan_members (
    clan_id    UUID NOT NULL REFERENCES clans (id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    joined_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (clan_id, user_id)
);

CREATE TABLE trials (
    id            UUID PRIMARY KEY,
    slug          TEXT NOT NULL UNIQUE,
    title         TEXT NOT NULL,
    difficulty    TEXT NOT NULL,
    topics        TEXT[] NOT NULL DEFAULT '{}',
    lifecycle     TEXT NOT NULL,
    version       INTEGER NOT NULL,
    -- BLAKE3 CID of the immutable Trial package in the data plane.
    content_cid   TEXT NOT NULL,
    published_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT trials_slug_shape CHECK (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$'),
    CONSTRAINT trials_version_positive CHECK (version >= 1)
);

CREATE INDEX trials_lifecycle_difficulty_idx ON trials (lifecycle, difficulty);

-- Merged local-first progress. One row per (user, content key); the client
-- pushes compact checkpoints, never per-interaction events.
CREATE TABLE progress_entries (
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    key          TEXT NOT NULL,
    revision     BIGINT NOT NULL,
    completion   TEXT NOT NULL,
    recorded_at  TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id, key)
);

CREATE TABLE trial_user_state (
    user_id            UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    trial_id           UUID NOT NULL REFERENCES trials (id) ON DELETE CASCADE,
    status             TEXT NOT NULL DEFAULT 'unsolved',
    attempts           INTEGER NOT NULL DEFAULT 0,
    -- Revealing published solutions is allowed. It forfeits rank credit only.
    revealed_solutions BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (user_id, trial_id)
);

-- One row per distinct solved Trial. Rank is recomputed from this set rather
-- than incremented in place, so replacing the ranking model is a recompute.
CREATE TABLE solves (
    user_id             UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    trial_id            UUID NOT NULL REFERENCES trials (id) ON DELETE CASCADE,
    difficulty          TEXT NOT NULL,
    lifecycle           TEXT NOT NULL,
    revealed_solutions  BOOLEAN NOT NULL,
    solved_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, trial_id)
);

CREATE TABLE submissions (
    id               UUID PRIMARY KEY,
    -- Immutable job identifier handed to the judge. Minted once, never reused.
    job_id           UUID NOT NULL UNIQUE,
    user_id          UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    trial_id         UUID NOT NULL REFERENCES trials (id) ON DELETE CASCADE,
    trial_version    INTEGER NOT NULL,
    source_cid       TEXT NOT NULL,
    -- Small tagged state document, not a blob.
    state            JSONB NOT NULL,
    verdict          TEXT,
    -- Hash of the full result manifest, which itself lives in the data plane.
    result_manifest_hash TEXT,
    idempotency_key  TEXT NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at      TIMESTAMPTZ,
    CONSTRAINT submissions_idempotent UNIQUE (user_id, idempotency_key)
);

CREATE INDEX submissions_user_trial_idx ON submissions (user_id, trial_id);

-- Job queue. A queued job has no lease; leasing sets worker_id and expiry.
CREATE TABLE job_queue (
    job_id            UUID PRIMARY KEY REFERENCES submissions (job_id) ON DELETE CASCADE,
    enqueued_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    worker_id         TEXT,
    -- Trust class of the leasing worker, recorded so a hidden-test dispatch
    -- decision is auditable after the fact.
    worker_trust      TEXT,
    lease_expires_at  TIMESTAMPTZ
);

CREATE INDEX job_queue_available_idx ON job_queue (enqueued_at)
    WHERE worker_id IS NULL;

CREATE TABLE events (
    id           UUID PRIMARY KEY,
    kind         TEXT NOT NULL,
    summary      TEXT NOT NULL,
    href         TEXT,
    subject      TEXT,
    occurred_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Recent is a 24-hour window over this table; Archive is the same table without
-- the window. Both read newest-first.
CREATE INDEX events_occurred_at_idx ON events (occurred_at DESC);
CREATE INDEX events_kind_occurred_at_idx ON events (kind, occurred_at DESC);
