FROM rust:1.58 as builder

WORKDIR /exchange-server

COPY . .

RUN cargo build

FROM debian:buster-slim

WORKDIR /exchange-server

COPY --from=builder /exchange-server/target/release/exchange-server .

CMD ["./exchange-server"]
