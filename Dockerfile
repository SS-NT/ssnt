FROM alpine as builder

RUN : \
    && apk upgrade \
    && apk add \
        curl \
        gcc \
        musl-dev \
        # Ctrl+C handler
        dumb-init \
    && curl https://sh.rustup.rs -sSf | sh -s -- --profile minimal --default-toolchain stable -y

WORKDIR /build

COPY Cargo.lock .
COPY Cargo.toml .

# TODO: do not rebuild deps if some of these change
COPY crates crates

# cache all dependencies. see the following for more detailed explanation:
# https://github.com/twilight-rs/http-proxy/blob/f7ea681fa4c47b59692827974cd3a7ceb2125161/Dockerfile#L40-L75
RUN : \
    && mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && source $HOME/.cargo/env \
    && cargo build --release --no-default-features \
    && rm -f target/release/deps/ssnt*

COPY src src

RUN : \
    && source $HOME/.cargo/env \
    && cargo build --release --no-default-features \
    && cp target/release/ssnt ssnt

FROM scratch

COPY --from=builder /build/ssnt .
COPY --from=builder /usr/bin/dumb-init .
COPY assets assets

ENTRYPOINT ["./dumb-init", "--", "./ssnt", "host"]
