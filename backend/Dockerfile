# syntax=docker/dockerfile:1
# Multi-stage build: cargo-chef caches dependency compilation so that
# `docker compose up --build` after a `git pull` only recompiles changed crates.

FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked --bin backend

FROM debian:trixie-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --create-home app \
 && mkdir -p /data && chown app:app /data
COPY --from=builder /app/target/release/backend /usr/local/bin/backend
USER app
# The backend opens sqlite://data.db relative to its working directory,
# so /data is the persisted volume.
WORKDIR /data
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/backend"]
