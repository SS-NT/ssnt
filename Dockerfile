FROM lukemathwalker/cargo-chef:0.1.62-rust-1.73.0-alpine AS chef
WORKDIR /build

FROM chef AS planner

RUN : \
    && apk add --no-cache \
        # Ctrl+C handler
        dumb-init

COPY src src
COPY crates crates
COPY Cargo.* .

RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --no-default-features --recipe-path recipe.json

COPY src src
COPY crates crates

RUN cargo build --release --no-default-features

FROM scratch

COPY --from=builder /build/target/release/ssnt .
COPY --from=planner /usr/bin/dumb-init .
COPY assets assets

ENTRYPOINT ["./dumb-init", "--", "./ssnt", "host"]
