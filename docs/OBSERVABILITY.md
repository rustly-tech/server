# Observability

Structured tracing from the first commit; a large metrics stack later, if and
when it earns its place.

## Correlation identifiers

Spelled identically everywhere. The constants live in
`rustly_common::telemetry::fields` and are pinned by a test, because these names
appear in dashboards and log queries.

| Field | Meaning |
| --- | --- |
| `request_id` | Per-request id, echoed in `x-request-id` |
| `user_ref` | Privacy-safe user reference (`u:` plus 8 characters), never the full id |
| `submission_id` | Submission |
| `job_id` | Judge job |
| `worker_id` | Judge worker |
| `trial_id` | Trial |
| `trial_version` | Trial content version, so a verdict traces to exact tests |

Raw user ids never appear in log lines. `Principal::log_ref` is the only way a
handler refers to a caller.

## Log format

`RUSTLY_LOG_FORMAT=json` selects machine-readable output; anything else gives
compact human output. Level comes from `RUST_LOG`, defaulting to `info`.

## Metrics we intend to expose

Interfaces first, collectors later. Nothing here is wired to a backend yet -
status `PLANNED`.

| Metric | Why it matters |
| --- | --- |
| API latency (p50/p95/p99) | Baseline health |
| Submission rate | Capacity planning |
| Judge queue depth | The first symptom of worker starvation |
| Judge wait p50/p95/p99 | What a user actually experiences |
| Compile latency | Dominates the submission path |
| Execution latency | Per-test cost |
| Cache hit rate | Whether the artifact cache is worth its complexity |
| Clean rebuild rate | Cache corruption or churn |
| Worker availability by trust class | Whether trusted capacity exists for hidden tests |
| CAS hit/miss | Data-plane effectiveness |
| P2P hit ratio | Whether P2P is actually saving egress |
| Database usage against quota | $0-mode headroom |
| Quota state | Input to the ZeroCostGovernor |

## What is deliberately not built yet

No Prometheus, no OpenTelemetry collector, no tracing backend, no dashboards.
Adding them before there is traffic would be infrastructure theatre with a
running cost.
