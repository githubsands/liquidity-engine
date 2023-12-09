FROM rust:1.57.0 as builder

WORKDIR /usr/src

COPY Cargo.toml .

COPY src ./src

RUN cargo build --release

FROM debian:buster-slim

COPY --from=builder /usr/src/target/release/orderbook-quoter-server /usr/local/bin

CMD ["orderbook-quoter-server"]
