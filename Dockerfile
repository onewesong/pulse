FROM rust:1.97-slim-bookworm AS builder

WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY templates ./templates
COPY web ./web

RUN cargo build --release --locked

FROM debian:bookworm-slim

ARG VERSION=dev

LABEL org.opencontainers.image.title="Pulse" \
      org.opencontainers.image.description="Lightweight Docker container metrics collector and dashboard" \
      org.opencontainers.image.source="https://github.com/onewesong/pulse" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="${VERSION}"

RUN mkdir -p /data

COPY --from=builder /src/target/release/pulse /usr/local/bin/pulse

ENV PULSE_DATABASE__PATH=/data/pulse.db \
    PULSE_WEB__LISTEN=0.0.0.0:8080 \
    PULSE_DOCKER__SOCKET=/var/run/docker.sock \
    PULSE_FILTERS__EXCLUDE_LABELS=pulse.ignore=true \
    RUST_LOG=pulse=info,tower_http=info

VOLUME ["/data"]
EXPOSE 8080
STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/local/bin/pulse"]
CMD ["serve"]
