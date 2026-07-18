FROM rust:1.86-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --bin brama

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/brama /usr/local/bin/brama
ENV RUST_LOG=info
ENV PORT=8080
EXPOSE 8080
CMD ["sh", "-c", "exec brama serve --port \"${PORT}\""]
