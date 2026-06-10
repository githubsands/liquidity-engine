# Orderbook Quoter Server

Builds a live orderbook from http snapshots from N configurable exchanges and then updates the orderbook 
through soft real time websocket depth updates. After a update the best ten asks and bids aswell as well 
as the spread are provided through a grpc server endpoint

## Libraries and components

# Exchange

Wrapper around exchange stream to handle websocket sinks and other functionality

# ExchangeStream

Runs both http snapshot streams and websocket streams. Can handle retriggering the http snapshot stream 
but it currently is not implemented in the Orderbook/DepthDriver. 

Future work: Ideally these streams are done purely on the stack but this must be verified. Correct
sequencing of orderbook snapshots and depth updates through their timestamps

# DepthDriver

Provides a controlling interface to all exchange streams that push depths.

#### Future work:

(1) Needs to handle orderbook reset and orderbook snapshot
retriggering with correct sequencing (https://github.com/binance/binance-spot-api-docs/blob/master/web-socket-streams.md#how-to-manage-a-local-order-book-correctly)

(2) Exchange Stream websocket failure states.

# Orderbook

features:

* `no_std`
* flattened stack only data structures
* contiguous
* precompile configurable fixed sized array level length by exchange connectivity count
* preinitialized 

// todo -- update my reasoning here ... on why i didn't just use a simple red black tree or 
   the previous [initial linked list idea](https://github.com/githubsands/liquidity-engine/pull/10)


# Quote GRPC Server

Takes the spread and provides the best ten deals and asks to a grpc client

# Depth Generator

Generates depths in many different sequences: upward, downward through
hacking a brownian motion stochastic process.

##### Future Work:

Oscillating Depths rather then just upward and downward trends

# Exchange Stubs

Provides both HTTP and websocket endpoints for depths. Leverages depth generator
as a dependency.

# Exchange Server

Dockerized exchange stub for full integration testing. Leverages exchange stub as a
dependency.

## Dependencies

Core dependencies for the orderbook quoter server.
