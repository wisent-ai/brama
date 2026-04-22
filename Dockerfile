FROM rust:1.86-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --bin model-router

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/model-router /usr/local/bin/model-router
ENV RUST_LOG=info
ENV PORT=8080
EXPOSE 8080
CMD ["sh", "-c", "exec model-router serve --port ${PORT}"]
