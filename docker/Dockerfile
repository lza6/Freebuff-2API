# Freebuff2API Rust 版 — 多阶段构建
FROM rust:1.85-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main(){}' > src/main.rs && echo '' > src/lib.rs && \
    cargo build --release 2>/dev/null || true

COPY src/ src/
RUN touch src/main.rs src/lib.rs && cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/freebuff2api /usr/local/bin/freebuff2api
ENV PORT=47821
VOLUME ["/data"]
EXPOSE 47821
ENTRYPOINT ["freebuff2api", "--config", "/data/config.json"]
