FROM rust:1.86-slim@sha256:57d415bbd61ce11e2d5f73de068103c7bd9f3188dc132c97cef4a8f62989e944 AS builder
RUN printf '%s\n' \
      'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/20260714T000000Z/ bookworm main' \
      'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/20260714T000000Z/ bookworm-updates main' \
      'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/20260714T000000Z/ bookworm-security main' \
      > /etc/apt/sources.list \
    && rm -f /etc/apt/sources.list.d/debian.sources \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
      pkg-config=1.8.1-1 \
      libssl-dev=3.0.20-1~deb12u2 \
      ca-certificates=20230311+deb12u1 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
ARG BRAMA_SOURCE_REVISION
ARG BRAMA_BUILD_PLATFORM
ARG BRAMA_BUILD_TIMESTAMP
RUN test -n "$BRAMA_SOURCE_REVISION" \
    && test -n "$BRAMA_BUILD_PLATFORM" \
    && test -n "$BRAMA_BUILD_TIMESTAMP"
RUN BRAMA_SOURCE_REVISION="$BRAMA_SOURCE_REVISION" \
    BRAMA_BUILD_PLATFORM="$BRAMA_BUILD_PLATFORM" \
    BRAMA_BUILD_TIMESTAMP="$BRAMA_BUILD_TIMESTAMP" \
    cargo build --locked --release --bin brama

FROM debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818
# Supply only the exact URL rendered by `stado storage url` for a pinned
# stado://releases/stado/... object; the checksum remains independently pinned.
ARG STADO_CLI_RELEASE_URL
ARG STADO_CLI_RELEASE_SHA256
RUN test -n "$STADO_CLI_RELEASE_URL" \
    && test -n "$STADO_CLI_RELEASE_SHA256" \
    && printf '%s\n' \
      'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/20260714T000000Z/ bookworm main' \
      'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/20260714T000000Z/ bookworm-updates main' \
      'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/20260714T000000Z/ bookworm-security main' \
      > /etc/apt/sources.list \
    && rm -f /etc/apt/sources.list.d/debian.sources \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
      ca-certificates=20230311+deb12u1 \
      libssl3=3.0.20-1~deb12u2 \
      curl=7.88.1-10+deb12u15 \
    && curl --fail --silent --show-error --location "$STADO_CLI_RELEASE_URL" --output /tmp/stado \
    && printf '%s  %s\n' "$STADO_CLI_RELEASE_SHA256" /tmp/stado | sha256sum --check - \
    && chmod a=rx /tmp/stado \
    && mv /tmp/stado /usr/local/bin/stado \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/brama /usr/local/bin/brama
ENV RUST_LOG=info
ENV PORT=8080
EXPOSE 8080
CMD ["sh", "-c", "exec brama serve --port \"${PORT}\""]
