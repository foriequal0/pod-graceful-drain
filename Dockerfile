FROM rust:1.79-slim-bookworm as fetch
RUN mkdir /src
WORKDIR /src
COPY ./Cargo.* /src/
RUN cargo fetch --verbose

FROM fetch as builder
RUN apt-get update && apt-get install -y git
COPY . /src/
RUN cargo install --path .

FROM debian:bookworm-slim
WORKDIR /app
COPY --from=builder /usr/local/cargo/bin/pod-graceful-drain /app/pod-graceful-drain
ENTRYPOINT ["/app/pod-graceful-drain"]
