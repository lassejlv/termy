# Build from the repository root: docker build -f crates/relay/Dockerfile .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p termy_relay

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/termy-relay /usr/local/bin/termy-relay
ENV PORT=8080
EXPOSE 8080
USER nobody
CMD ["termy-relay"]
