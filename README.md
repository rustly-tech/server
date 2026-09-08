# rustly-tech/core

The **trusted control plane** for [Rustly](https://rustly.tech). Rust, Tokio,
Axum, SQLx, PostgreSQL.

## What this service owns

It is deliberately small. It is the only writer of:

- identity and permissions
- Rank, Level, Global Rank
- accepted-verdict state
- hidden-test dispatch decisions
- supporter entitlement and moderation state

Everything else in Rustly can be recomputed, re-fetched, or reconstructed.

## What this service does not do

| Not this | Where it lives instead |
| --- | --- |
| Serve lessons, cheatsheets, Trial statements | Static content, addressed by CID |
| Carry source, tests, or artifacts | The content-addressed data plane |
| Execute user code | The judge, behind a pull-based broker |
| Store blobs in PostgreSQL | Nothing over 64 KiB reaches this API at all |

The 64 KiB body limit is the mechanical enforcement of the last point, and there
is a test that asserts it.

## Status

| Component | Status |
| --- | --- |
| Domain model, ranking, progress merge, verdict rules | **IMPLEMENTED**, unit tested |
| HTTP API v1 (all endpoints in [`docs/API.md`](docs/API.md)) | **IMPLEMENTED**, integration tested end to end |
| In-memory metadata store | **IMPLEMENTED**, passes the conformance suite |
| PostgreSQL metadata store | **IMPLEMENTED**, passes the same conformance suite in CI |
| Judge broker protocol v1 | **IMPLEMENTED** on this side; see `rustly-tech/judge` for the worker |
| SSE submission stream | **IMPLEMENTED** |
| Ranking model `provisional-v0` | **EXPERIMENTAL** and labelled as such by the API |
| Q&A persistence, moderation, clan management writes | **PLANNED** |
| Payments and supporter billing | **PLANNED**, deliberately not started |

No component here is described as production-ready that is not covered by tests.

## Layout

```
crates/
  common/      errors, typed ids, time, telemetry field names
  domain/      pure product rules: users, trials, verdicts, milestones, events
  protocol/    versioned wire contracts (API v1, judge broker v1)
  auth/        HMAC bearer tokens, scopes, principals, trust classes
  storage/     MetadataStore trait + memory and PostgreSQL backends + conformance suite
  ranking/     replaceable ranking models and the level curve
  progress/    local-first checkpoint merge (idempotent, commutative, monotonic)
  community/   Recent/Archive feed, Q&A types
services/
  api/         the Axum service
migrations/    PostgreSQL schema
```

`crates/domain` has no I/O, no async, and no database. Every product rule that
matters lives there and is directly testable.

## Running it

```sh
# In-memory, seeded with the Ownership vertical slice. No database needed.
cargo run -p rustly-api

curl -s localhost:8080/api/version | jq
curl -s localhost:8080/api/v1/trials | jq
curl -s localhost:8080/api/v1/users/ferris | jq
```

With PostgreSQL:

```sh
docker run -d --name rustly-pg -e POSTGRES_PASSWORD=rustly -e POSTGRES_DB=rustly \
  -p 5432:5432 postgres:17-alpine

DATABASE_URL=postgres://postgres:rustly@localhost:5432/rustly \
RUSTLY_TOKEN_SECRET=a-development-secret-of-at-least-32-bytes \
cargo run -p rustly-api --features postgres
```

Migrations are applied automatically on start.

### Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `RUSTLY_BIND` | `0.0.0.0:8080` | Listen address |
| `DATABASE_URL` | unset | PostgreSQL URL. Unset selects the in-memory backend |
| `RUSTLY_TOKEN_SECRET` | required with a database | Token signing key, >= 32 bytes |
| `RUSTLY_BUILD` | `dev` | Build id reported by `/api/version` |
| `RUSTLY_LOG_FORMAT` | human | `json` for machine-readable logs |
| `RUSTLY_SEED_SLICE` | `1` | Seed the Ownership slice into the in-memory store |
| `RUST_LOG` | `info` | Tracing filter |

Without a database the service refuses to invent a persistent signing key; it
warns loudly and uses an ephemeral one, so tokens die with the process.

## Tests

```sh
cargo test --workspace                      # hermetic; no database required
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
```

The PostgreSQL conformance run needs a server:

```sh
RUSTLY_TEST_DATABASE_URL=postgres://postgres:rustly@localhost:5432/rustly_test \
  cargo test -p rustly-storage --features postgres,testing --test postgres_conformance
```

### Why the API tests use the in-memory backend

`crates/storage/src/conformance.rs` is a single suite that both backends must
pass identically, and CI runs it against a real `postgres:17` service container.
That equivalence is what makes it legitimate for the HTTP integration tests to
run in memory: they stay hermetic and fast without testing a fiction.

Both backends share the accepted-result decision itself - it is a pure function
in `crates/storage/src/finalize.rs` - so the two implementations cannot drift on
the rules that matter (idempotency, first-solve detection, system-fault
handling, rank recomputation).

## Database

PostgreSQL, standard SQL only. Runs against `postgres:17` in CI and against a
Neon branch in production; no business logic depends on which.

`sqlx` is used through the runtime `query` API rather than the compile-time
macros: the macros need a live database at *build* time, which would make
`cargo build` fail on a laptop with no database and turn every CI job into a
database job.

## Documentation

- [API v1 reference](docs/API.md)
- [Observability](docs/OBSERVABILITY.md)
- [Architectural invariants](https://github.com/rustly-tech/.github/blob/main/docs/ARCHITECTURE_INVARIANTS.md)
- [System architecture](https://github.com/rustly-tech/infra/blob/main/docs/ARCHITECTURE.md)

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
