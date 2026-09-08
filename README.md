# Rustly server

The Rustly server.

It provides the API for accounts, learning progress, rankings, submissions,
and community features. Exercise execution lives in the separate
[`judge`](https://github.com/rustly-tech/judge) project.

## Run locally

The default configuration uses an in-memory database and includes the Ownership
lesson used by the first working product slice.

```sh
cargo run -p rustly-api

curl -s localhost:8080/api/version | jq
curl -s localhost:8080/api/v1/trials | jq
```

See [development notes](docs/DEVELOPMENT.md) for PostgreSQL and configuration.

## Repository guide

- [`crates/domain`](crates/domain) contains Rustly's product rules.
- [`crates/protocol`](crates/protocol) contains the versioned API types.
- [`crates/storage`](crates/storage) contains the in-memory and PostgreSQL data stores.
- [`services/api`](services/api) contains the HTTP service.
- [`docs/API.md`](docs/API.md) documents the API.
- [`docs/OBSERVABILITY.md`](docs/OBSERVABILITY.md) documents logs and metrics.

## Verify

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Rustly is in early development. Implemented behavior is covered by the repository
test suite; planned work is identified in issues and documentation.

## License

MIT or Apache-2.0, at your option.
