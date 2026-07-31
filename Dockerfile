# Stage 1: Build
FROM rust:1.80-bookworm as builder
WORKDIR /build
COPY . .
RUN cargo build --release -p polkagent-cli

# Stage 2: Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/polkagent /usr/local/bin/polkagent
RUN useradd -r -s /bin/false polkagent
USER polkagent
VOLUME /data
ENV POLKAGENT_DATABASE_SQLITE_PATH=/data/polkagent.db
EXPOSE 9090
ENTRYPOINT ["polkagent"]
CMD ["serve"]
