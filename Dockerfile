# syntax=docker/dockerfile:1

# Stage 1: Build with a toolchain compatible with the locked dependency graph.
# The workspace's historical 1.80 MSRV declaration is tracked separately; the
# current lock includes crates requiring Rust 1.89, so the image must not use
# Cargo 1.80 and fail before compilation starts.
FROM rust:1.91-bookworm AS builder
WORKDIR /build
COPY . .
RUN --mount=type=cache,id=polkagent-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=polkagent-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=polkagent-target,target=/build/target,sharing=locked \
    find crates -type f -exec touch {} + \
    && touch Cargo.toml Cargo.lock \
    && cargo build --release --locked -p polkagent-cli \
    && install -D /build/target/release/polkagent /out/polkagent

# Stage 2: Runtime
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /out/polkagent /usr/local/bin/polkagent
RUN useradd --system --create-home --home-dir /home/polkagent --shell /usr/sbin/nologin polkagent \
    && install -d -o polkagent -g polkagent /data
USER polkagent
VOLUME /data
ENV POLKAGENT_DATABASE_SQLITE_PATH=/data/polkagent.db
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=6 \
    CMD ["curl", "--fail", "--silent", "--show-error", "http://127.0.0.1:8080/health/ready"]
ENTRYPOINT ["/usr/local/bin/polkagent"]
CMD ["serve", "--host", "0.0.0.0", "--port", "8080"]
