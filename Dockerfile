# Rustly control-plane API.
#
# Two-stage build onto a distroless base. The API is a trusted control plane and
# never executes user code, so it needs no toolchain, no shell, and no package
# manager at runtime.

FROM rust:1.98-bookworm AS build
WORKDIR /src

# Copy manifests first so dependency compilation caches across source edits.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY services ./services
COPY migrations ./migrations

ARG BUILD=dev
ENV RUSTLY_BUILD=${BUILD}
RUN cargo build --release --locked -p rustly-api --features postgres \
    && strip target/release/rustly-api

FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=build /src/target/release/rustly-api /app/rustly-api

ARG BUILD=dev
ENV RUSTLY_BUILD=${BUILD} \
    RUSTLY_BIND=0.0.0.0:8080 \
    RUSTLY_LOG_FORMAT=json \
    RUST_LOG=info

EXPOSE 8080
USER nonroot:nonroot
ENTRYPOINT ["/app/rustly-api"]
