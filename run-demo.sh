
$(pwd)/target/debug/orderbook-quoter-server --config /etc/usa-config.json
sleep 2
export ORDERBOOK_QUOTER_SERVER_URI="[::1]:9090"
$(pwd)/market-maker/target/debug/market-maker
