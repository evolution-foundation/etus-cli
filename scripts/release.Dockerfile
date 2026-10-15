# syntax=docker/dockerfile:1
ARG RUST_VERSION=1.93
FROM rust:${RUST_VERSION}-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo build --release --locked && cp target/release/etus /etus

FROM scratch
COPY --from=build /etus /etus
