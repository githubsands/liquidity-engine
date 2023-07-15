# Orderbook Quoter Server 

# High level

Builds a live orderbook from http snapshots from N configurable exchanges and updates it through
websocket depth updates. After a update the best ten asks and bids aswell as well as the spread 
are provided through a grpc server endpoint

# Lower level

IO components: ExchangeStreams, and Quote GRPC Server are ran on seperate tokio runtimes in their own
pinned threads.

Orderbook is ran with multiplie threads. One for writing to the book the others for reading it.

# Configuration

Exchange boots through a config by running `./orderbook-quoter-server --config=$(CONFIG_LOCATION)`. The 
amount of exchanges in the exchange array must be equal to the orderbook's `exchange_count`. Every
`depth` field must be equal in the exchanges and orderbook sections.

```       
exchanges:
  - client_name: "binance_usa_1"
    exchange_name: 1
    snapshot_enabled: true
    http_client: true
    snapshot_uri: "http://localhost:5000"
    ws_uri: "wss://localhost:5001"
    ws_poll_rate_milliseconds: 99
    ws_presequenced_depth_buffer: 4000,
    depth: 5000
    buffer_size: 6000
- client_name: "binance_usa_2"
    exchange_name: 2
    snapshot_enabled: true
    http_client: true
    snapshot_uri: "http://localhost:6000"
    ws_uri: "wss://localhost:6001"
    ws_poll_rate_milliseconds: 99
    ws_presequenced_depth_buffer: 4000,
    depth: 5000
    buffer_size: 6000
orderbook:
  exchange_count: 2
  depth: 5000
  tick_size: 0.01
  mid_price: 2700
  ticker: "BTCUSDT"
  ring_buffer:
    ring_buffer_size: 1024
    channel_buffer_size: 512
grpc_server:
  host_uri: "127.0.0.1:5000"
```

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
but it currently is not implemented in the Orderbook/DepthDriver. 

Future work: Ideally these streams are done purely on the stack but this must be verified. Correct
sequencing of orderbook snapshots and depth updates through their timestamps

### Exchange

Wrapper around exchange stream to handle websocket sinks and other functionality

### DepthDriver

Provides a controlling interface to all exchange streams. 

Future work: 

(1) Needs to handle orderbook reset and orderbook snapshot
retriggering with correct sequencing

(2) Exchange Stream websocket failure states.

### Orderbook

Handles orderbook writing and reading.  

Has 2 states:

(1) Building the orderbook with http snapshots

(2) Updating the the orderbook with ws depths and then reading the orderbook for best deals

Future work: 

(1) A state when the orderbook is needs rebuilding if a ExchangeStream websocket connection fails. 

(2) Reduce dynamic memory allocations

(3) Different thread scheme for orderbook reading - currently it's thread spins up child threads for 
ask and bid traversal. By default this should have a threadpool though so we don't waste time making new threads.

(4) Use a decimals or another solution over floats for quantities.

(5) Use a decimals or another solution over floats for price levels.

(6) Possibly more performant atomic memory ordering 

### Testing

#### 1. Depth Generator

Generates depths in many different sequences.

Future Work:

Oscillating Depths rather then just upward and downward trends

#### 2. Exchange Stubs

Provides both HTTP and websocket endpoints for depths. Leverages depth generator.

#### 3. Exchange Server

Dockerized exchange stub for full integration testing.

### Quote GRPC Server

Takes the spread and provides the best ten deals and asks to a grpc client

Future work:

TBD
