FROM rust:1.84-slim-bookworm AS cache

# prepare git cli
RUN apt-get update && apt-get install -y git

# create /src dir
RUN mkdir /src
WORKDIR /src

# warm-up dependencies build cache
COPY ./Cargo.* /src/
COPY build.rs /src/
RUN mkdir src && \
    echo 'fn main() { println!("Hello, world!"); }' > src/main.rs && \
    cargo build && \
    rm -rf src

FROM cache AS build

COPY . /src/
RUN cargo build

FROM debian:bookworm-slim
WORKDIR /app
COPY --from=build /src/target/debug/pod-graceful-drain /app/pod-graceful-drain
ENTRYPOINT ["/app/pod-graceful-drain"]
