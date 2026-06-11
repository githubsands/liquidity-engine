# Orderbook Quoter Server

Builds a live orderbook from http snapshots from N configurable exchanges and then updates the orderbook 
through soft real time websocket depth updates. After a update the best ten asks and bids aswell as well 
as the spread are provided through a grpc server endpoint

## Libraries and components

# Orderbook

features

* `no_std`
* flattened stack only hashmap where each price point (or level) is a key and its value is a 
   array of exchange liquidity nodes
* contiguous
* precompile configurable fixed sized array level length by exchange connectivity count
* preinitialized 
* `O(1) + O(EC)` price point reads and writes time complexity where `EC` = `exchange count`

// previous [initial linked list idea](https://github.com/githubsands/liquidity-engine/pull/10) here from 2023

# Depth Pool

features

* zero copy serialization for depth updates preallocated arena
* preallocated arena

# DepthDriver

features

* concurrent network io from `io uring` through `compio's` executor

# ExchangeStream

Runs both http snapshot streams and websocket streams. Can handle retriggering the http snapshot stream 
but it currently is not implemented in the Orderbook/DepthDriver. 

Future work: Ideally these streams are done purely on the stack but this must be verified. Correct
sequencing of orderbook snapshots and depth updates through their timestamps


#### Future work:

(1) Needs to handle orderbook reset and orderbook snapshot
retriggering with correct sequencing (https://github.com/binance/binance-spot-api-docs/blob/master/web-socket-streams.md#how-to-manage-a-local-order-book-correctly)

(2) Exchange Stream websocket failure states.

# Deal worker

revamp / work in progress - takes the spread and provides the best ten deals through a grpc server 

# Depth Generator



Generates depths in many different sequences: upward, downward through
hacking a brownian motion stochastic process.

##### Future Work:

Oscillating Depths rather then just upward and downward trends

# Exchange stubs and server

Provides both HTTP and websocket endpoints for depths. Leverages depth generator
as a dependency.

Dockerized exchange stub for full integration testing. Leverages exchange stub as a
dependency.
