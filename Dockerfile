FROM rust:slim-bookworm as builder
WORKDIR /usr/src/app
RUN apt-get update && apt-get install -y libssl-dev pkg-config && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /usr/src/app/target/release/audiobookshelf-discord-rpc /app/
VOLUME /app/config
VOLUME /run/user/1000/discord-ipc-0

CMD ["./audiobookshelf-discord-rpc", "-c", "/app/config/config.json"] 