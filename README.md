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

features

* asynchronous 
* orderbook snap shots through http
* real time depth updates

# Deal worker

revamp / work in progress - takes the spread and provides the best ten deals through a grpc server 

# Depth Generator

features

*  depth update generation in many different sequences: upward, downward by
hacking a brownian motion stochastic process

# Exchange stubs and server

Provides both HTTP and websocket endpoints for depths. Leverages depth generator
as a dependency.

Dockerized exchange stub for full integration testing. Leverages exchange stub as a
dependency.
