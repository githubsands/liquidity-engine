FROM rust:1.58 as builder

WORKDIR /market-maker

COPY . . 

RUN cargo build

FROM debian:buster-slim

WORKDIR /market-maker

COPY --from=builder /exchange-server/target/release/market-maker .

CMD ["./market-maker"]
