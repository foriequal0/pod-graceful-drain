FROM rust:1.79-slim-bookworm as builder
RUN apt-get update && apt-get install -y git
RUN mkdir /src
WORKDIR /src
COPY . /src/
RUN cargo install --path .

FROM debian:bookworm-slim
WORKDIR /app
COPY --from=builder /usr/local/cargo/bin/pod-graceful-drain /app/pod-graceful-drain
ENTRYPOINT ["/app/pod-graceful-drain"]
