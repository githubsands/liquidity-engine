FROM rust:1.55 as builder

WORKDIR /market-maker

COPY Cargo.toml Cargo.lock ./

RUN cargo build --release --locked

COPY src ./src

RUN cargo build --release --locked

FROM debian:buster-slim

WORKDIR /market-maker

COPY --from=builder /market-maker/target/release/market-maker .

CMD ["./market-maker"]
