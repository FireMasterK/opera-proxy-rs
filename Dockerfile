FROM rust:slim AS build

WORKDIR /app

RUN --mount=type=cache,target=/var/cache/apt \
    apt-get update && \
    apt-get install -y --no-install-recommends \
    build-essential \
    cmake \
    clang \
    git \
    perl \
    pkg-config \
    ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked && \
    cp target/release/opera-proxy-rs /app/opera-proxy-rs

FROM debian:stable-slim

RUN --mount=type=cache,target=/var/cache/apt \
    apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=build /app/opera-proxy-rs /app/opera-proxy-rs

EXPOSE 18080

ENTRYPOINT ["/app/opera-proxy-rs"]
CMD ["--bind-address", "0.0.0.0:18080"]
