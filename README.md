# <span style="color: #FF6B6B;">Orderbook Quoter Server</span>

## <span style="color: #4ECDC4;">High Level</span>

Builds a live orderbook from http snapshots from N configurable exchanges and then updates the orderbook 
through soft real time websocket depth updates. After a update the best ten asks and bids aswell as well 
as the spread are provided through a grpc server endpoint

## <span style="color: #45B7D1;">Lower Level</span>

IO components: ExchangeStreams, and Quote GRPC Server are ran on seperate tokio runtimes in their own
pinned threads.

// todo: reason about the cores here and seek alternative setup 
   -- too much information is being exchanged between cores during real time processing

Orderbook is ran with multiplie threads. One for writing to the book the others for reading it.

### <span style="color: #96CEB4;">Orderbook Structure</span>

// todo -- update my reasoning here ... on why i didn't just use a simple red black tree
    or the previous [initial linked list idea](https://github.com/githubsands/liquidity-engine/pull/10)

## <span style="color: #FFEAA7;">Configuration</span>

Exchange boots through a config by running `./orderbook-quoter-server --config=$(CONFIG_LOCATION)`. The 
amount of exchanges in the exchange array must be equal to the orderbook's `exchange_count`. Every
`depth` field must be equal in the exchanges and orderbook's depth field should cover the entire 
expected trading range for the lifetime of this service. 

The larger expected volatility the higher the orderbook's depth needs to be.

```yaml       
exchanges:
  - client_name: "binance_usa_1"
    exchange_name: 0
    snapshot_enabled: true
    http_client: true
    snapshot_uri: "http://localhost:5000"
    ws_uri: "wss://localhost:5001"
    ws_poll_rate_milliseconds: 99
    ws_presequenced_depth_buffer: 4000,
    depth: 5000
    buffer_size: 6000
- client_name: "binance_usa_2"
    exchange_name: 1
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

## <span style="color: #74B9FF;">Building</span>

Builds are done through the build script (build.rs). The script reads the config file and runs procedural macros on 
the code base. The config file's "orderbook.exchange_count" and exchange's exchange array length must be equal or the
build script will fail.

Deployment team work flow goes as this: (1) edit the `config.yaml` with necessary exchanges and then run the build script 
from the root directory (cargo build).

Files changed by the build script are: 

(1) orderbook/src/lib.rs

## <span style="color: #A29BFE;">Components</span>

### <span style="color: #FD79A8;">1. ExchangeStream</span>

Runs both http snapshot streams and websocket streams. Can handle retriggering the http snapshot stream 
but it currently is not implemented in the Orderbook/DepthDriver. 

Future work: Ideally these streams are done purely on the stack but this must be verified. Correct
sequencing of orderbook snapshots and depth updates through their timestamps

### <span style="color: #FDCB6E;">2. Exchange</span>

Wrapper around exchange stream to handle websocket sinks and other functionality

### <span style="color: #6C5CE7;">3. DepthDriver</span>

Provides a controlling interface to all exchange streams that push depths.

#### Future work:

(1) Needs to handle orderbook reset and orderbook snapshot
retriggering with correct sequencing (https://github.com/binance/binance-spot-api-docs/blob/master/web-socket-streams.md#how-to-manage-a-local-order-book-correctly)

(2) Exchange Stream websocket failure states.

### <span style="color: #E17055;">4. Orderbook</span>

Handles orderbook writing and reading.  

Has 2 states:

(1) Building the orderbook with http snapshots

(2) Updating the the orderbook with ws depths and then reading the orderbook for best deals

### <span style="color: #00CEC9;">5. Quote GRPC Server</span>

Takes the spread and provides the best ten deals and asks to a grpc client

#### Future work:

TBD

#### Future work:

(1) A state when the orderbook is needs rebuilding if a ExchangeStream websocket connection fails. 

(2) Reduce dynamic memory allocations

(3) Seperate bid and ask depth updates into two buffers and write from two different threads rather then one.

(4) Use a decimals or another solution over floats for quantities.

(5) Use a decimals or another solution over floats for price levels.

(6) Possibly more performant atomic memory ordering 

## <span style="color: #FF7675;">Testing</span>

### <span style="color: #55A3FF;">1. Depth Generator</span>

Generates depths in many different sequences: upward, downward through
hacking a brownian motion stochastic process.

##### Future Work:

Oscillating Depths rather then just upward and downward trends

### <span style="color: #FD79A8;">2. Exchange Stubs</span>

Provides both HTTP and websocket endpoints for depths. Leverages depth generator
as a dependency.

### <span style="color: #FDCB6E;">3. Exchange Server</span>

Dockerized exchange stub for full integration testing. Leverages exchange stub as a
dependency.

## <span style="color: #E84393;">Dependencies</span>

Core dependencies for the orderbook quoter server.
