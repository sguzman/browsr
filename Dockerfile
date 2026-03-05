FROM rust:1.90-alpine AS builder

WORKDIR /app

# Build static binary for a tiny runtime image.
RUN apk add --no-cache musl-dev && rustup target add x86_64-unknown-linux-musl

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY config ./config

RUN cargo build --release --locked --target x86_64-unknown-linux-musl && \
    strip target/x86_64-unknown-linux-musl/release/browsr

FROM scratch

WORKDIR /app

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/browsr /browsr
COPY --from=builder /app/config/server.toml /config/server.toml

EXPOSE 17373

ENV BROWSR_CONFIG=/config/server.toml

ENTRYPOINT ["/browsr"]
