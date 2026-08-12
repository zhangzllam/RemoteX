# syntax=docker/dockerfile:1.7

FROM rust:1.85-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p remotex-control -p remotex-relay

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin remotex
USER 10001:10001
WORKDIR /app

FROM runtime AS control
COPY --from=builder /src/target/release/remotex-control /usr/local/bin/remotex-control
EXPOSE 8080
HEALTHCHECK --interval=15s --timeout=3s --start-period=15s --retries=3 \
  CMD curl --fail --silent http://127.0.0.1:8080/ready >/dev/null || exit 1
ENTRYPOINT ["/usr/local/bin/remotex-control"]

FROM runtime AS relay
COPY --from=builder /src/target/release/remotex-relay /usr/local/bin/remotex-relay
EXPOSE 7443/udp 8081
HEALTHCHECK --interval=15s --timeout=3s --start-period=10s --retries=3 \
  CMD curl --fail --silent http://127.0.0.1:8081/health >/dev/null || exit 1
ENTRYPOINT ["/usr/local/bin/remotex-relay"]
