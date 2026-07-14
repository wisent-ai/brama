FROM rust:1.86-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --bin brama

# Runtime: node:22-slim has node + npm preinstalled on debian bookworm so we
# can install the CLI agents (claude-code, codex, opencode via npm; kimi-cli
# via uv). Those CLIs are what back the /v1/chat/completions models whose
# names end in -subscription.
FROM node:22-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl git python3 python3-pip \
    && rm -rf /var/lib/apt/lists/*

# npm-installed coding CLIs
# The @openai/codex package is a JS wrapper that requires a platform-specific
# optional dependency (e.g. @openai/codex-linux-x64). npm sometimes skips it in
# global installs, so install the native binary explicitly via npm alias.
RUN npm install -g \
      @anthropic-ai/claude-code \
      "@openai/codex-linux-x64@npm:@openai/codex@linux-x64" \
      @openai/codex \
      opencode-ai \
      @moonshot-ai/kimi-code \
    && npm cache clean --force

COPY --from=builder /build/target/release/brama /usr/local/bin/brama
COPY scripts/refresh-clis.sh /usr/local/bin/refresh-clis.sh
COPY scripts/sync-subscription-catalog.mjs /usr/local/bin/sync-subscription-catalog.mjs
RUN chmod +x /usr/local/bin/refresh-clis.sh /usr/local/bin/sync-subscription-catalog.mjs
ENV RUST_LOG=info
ENV PORT=8080
EXPOSE 8080
# Keep the subscription CLIs current at runtime. The claude-code / codex /
# opencode CLIs are installed unpinned at build time, so an image baked weeks
# ago carries a stale CLI. When the upstream provider changes its auth (as
# Anthropic did 2026-05-27), that stale CLI starts returning 401 for every
# token and the pool burns until someone manually rebuilds. A background loop
# re-pulls the latest CLIs on startup and every 6h (best-effort: failures keep
# the current version), so every running instance self-heals to the newest CLI
# without a manual rebuild. The server starts immediately via exec; the refresh
# never blocks startup or the TCP health probe.
#
# refresh-clis.sh stages each update into an inactive slot and atomically flips
# the symlinks, so the live /usr/local/bin/claude the dispatcher spawns is
# NEVER unlinked mid-update (the old in-place `npm install -g @latest` caused
# `spawn ENOENT` on cold containers serving traffic during the reinstall).
CMD ["sh", "-c", "(while true; do /usr/local/bin/refresh-clis.sh || true; sleep 21600; done) & (while true; do /usr/local/bin/sync-subscription-catalog.mjs || true; sleep 21600; done) & exec brama serve --port ${PORT}"]
