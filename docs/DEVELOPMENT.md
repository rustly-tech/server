# Server development

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `RUSTLY_BIND` | `0.0.0.0:8080` | Listen address |
| `DATABASE_URL` | unset | PostgreSQL URL; unset uses the in-memory store |
| `RUSTLY_TOKEN_SECRET` | required with a database | Token signing key of at least 32 bytes |
| `RUSTLY_ARTIFACT_TOKEN_SECRET` | required with a database | Upload grant/receipt key shared with the artifact gateway |
| `RUSTLY_ARTIFACT_GATEWAY_URL` | `http://127.0.0.1:8081` | Browser-visible artifact gateway base URL |
| `RUSTLY_BUILD` | `dev` | Build identifier returned by `/api/version` |
| `RUSTLY_LOG_FORMAT` | `human` | Use `json` for structured logs |
| `RUSTLY_SEED_SLICE` | `1` | Seed the Ownership product slice in memory |
| `RUST_LOG` | `info` | Tracing filter |

Without a database, the server creates an ephemeral signing key and tokens expire
when the process exits.

## PostgreSQL

```sh
docker run -d --name rustly-pg \
  -e POSTGRES_PASSWORD=rustly \
  -e POSTGRES_DB=rustly \
  -p 5432:5432 \
  postgres:17-alpine

DATABASE_URL=postgres://postgres:rustly@localhost:5432/rustly \
RUSTLY_TOKEN_SECRET=a-development-secret-of-at-least-32-bytes \
RUSTLY_ARTIFACT_TOKEN_SECRET=a-separate-development-secret-32-bytes \
cargo run -p rustly-api --features postgres
```

Migrations run when the server starts. The PostgreSQL and in-memory stores share
a conformance suite. Run the PostgreSQL portion with:

```sh
RUSTLY_TEST_DATABASE_URL=postgres://postgres:rustly@localhost:5432/rustly_test \
  cargo test -p rustly-storage --features postgres,testing \
  --test postgres_conformance
```

The HTTP integration tests use the in-memory store. The shared conformance suite
checks that both stores follow the same product rules.

## Worker credentials

Operator-managed judge workers authenticate with a token whose trust class is
part of the signed claims. Issue one with the same signing secret used by the
API, then place the output in the worker's secret store:

```sh
RUSTLY_TOKEN_SECRET=the-same-secret-used-by-the-api \
  cargo run -q -p rustly-api --bin rustly-worker-token -- \
  ownership-worker-1 trusted 2592000
```

The command prints only the token. A worker cannot promote itself by changing
the trust class in its lease request.
