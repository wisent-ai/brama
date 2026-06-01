FROM rust:1.86-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --bin model-router

# Runtime: node:22-slim has node + npm preinstalled on debian bookworm so we
# can install the CLI agents (claude-code, codex, opencode via npm; kimi-cli
# via uv). Those CLIs are what back the /v1/chat/completions models whose
# names end in -subscription.
FROM node:22-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl git python3 python3-pip \
    && rm -rf /var/lib/apt/lists/*

# npm-installed coding CLIs
RUN npm install -g \
      @anthropic-ai/claude-code \
      @openai/codex \
      opencode-ai \
    && npm cache clean --force

# uv + kimi-cli (kimi-cli is a Python package, not npm)
ENV UV_TOOL_DIR=/opt/uv-tools
ENV UV_TOOL_BIN_DIR=/usr/local/bin
ENV UV_PYTHON_INSTALL_DIR=/opt/uv-python
RUN curl -fsSL https://astral.sh/uv/install.sh | sh \
    && /root/.local/bin/uv tool install --python 3.13 kimi-cli \
    && chmod -R a+rX /opt/uv-tools /opt/uv-python

COPY --from=builder /build/target/release/model-router /usr/local/bin/model-router
COPY scripts/refresh-clis.sh /usr/local/bin/refresh-clis.sh
RUN chmod +x /usr/local/bin/refresh-clis.sh
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
CMD ["sh", "-c", "(while true; do /usr/local/bin/refresh-clis.sh || true; sleep 21600; done) & exec model-router serve --port ${PORT}"]
