FROM rust:1.55 as builder

WORKDIR /orderbook-quoter-server

COPY Cargo.toml Cargo.lock ./

RUN cargo build --release --locked

COPY src ./src

RUN cargo build --release --locked

FROM debian:buster-slim

WORKDIR /orderbook-quoter-server

COPY --from=builder /orderbook-quoter-server/target/release/orderbook-quoter-server .

CMD ["./orderbook-quoter-server"]
