# orderbook-quoter-server

## high level

orderbook-quoter-server pulls in exchange orderbook depths through websockets,
updates an in memory orderbook, and then calculates quotes to be submitted
through a grpc server

## low level

all io related activity is done on 1 thread through leveraging the tokio runtime,
while orderbook rebalancing and quoting are done on other threads.
