# Orderbook Quoter Server 

# High level

Builds a live orderbook from http snapshots from N configurable exchanges and updates it through
websocket depth updates. After a update the best ten asks and bids aswell as well as the spread 
are provided through a grpc server endpoint

# Lower level

IO components, ExchangeStreams, and Quote GRPC Server, are ran on seperate tokio runtimes in their own
pinned threads.

Orderbook is ran with multiplie threads. One for writing to the orderbook the others for reading the orderbook

# Building

Builds are done through the build script (build.rs). The script reads the config file and runs procedural macros on 
the code base. The config file's "orderbook.exchange_count" and exchange's exchange array length must be equal or the
build script will fail.

Deployment team work flow goes as this: (1) edit the `config.yaml` with necessary exchanges and then run the build script 
from the root directory (cargo build).

Files changed by the build script are: 

(1) orderbook/src/lib.rs

## Components 

### ExchangeStream

Runs both http snapshot streams and websocket streams. Can handle retriggering the http snapshot stream 
but it currently is not implemented in the Orderbook/ExchangeController. 

Future work: Ideally these streams are done purely on the stack but this must be verified. Correct 
sequencing of orderbook snapshots and depth updates through their timestamps

### ExchangeController

Provides a controlling interface to all exchange streams. 

Future work: 

(1) Handle more then just two exchanges (currently the orderbook can be configured for N exchanges but not the exchange controller)

(2) Needs to handle orderbook reset and orderbook snapshot
retriggering with correct sequencing, and websocket reconnection.

### Orderbook

Handles orderbook writing and reading.  

Has 2 states:

(1) Building the orderbook with http snapshots

(2) Updating the the orderbook with ws depths and then reading the orderbook for best deals

Future work: 

(1) A state when the orderbook is needs rebuilding if a ExchangeStream websocket connection fails. 

(2) Reduce dynamic memory allocations

(3) Possibly run ask and bid reader threads in their own threads rather
then having both readers run on the same core (a threadpool also would be another lower dev cost solution here)

(4) Use a decimals or another solution over floats for quantities.

(5) Use a decimals or another solution over floats for price levels.

(6) Possibly more performant atomic memory ordering 

### Testing

#### 1. Depth Generator

Generates depths in many different sequences.

#### 2. Exchange Stubs

Provides both HTTP and websocket endpoints for depths. Leverages depth generator.

#### 3. Exchange Server

Dockerized exchange stub for full integration testing.

### Quote GRPC Server

Takes the spread and provides the best ten deals and asks to a grpc client

Future work:

TBD
