FROM clux/muslrust:stable as builder


COPY Cargo.lock .
COPY Cargo.toml .
COPY src ./src

RUN cargo build --target x86_64-unknown-linux-musl --release && \
    cp target/x86_64-unknown-linux-musl/release/freezer /

FROM alpine

COPY --from=builder /freezer /bin/
