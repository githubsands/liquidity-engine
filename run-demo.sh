export ORDERBOOK_QUOTER_SERVER_URI="[::1]:9090"
./bin/orderbook-quoter-server/debug/orderbook-quoter-server --config /etc/usa-config.json
sleep 2
./bin/market-maker/target/debug/market-maker
