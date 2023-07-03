# Orderbook Quoter Server 

# High level

Builds a live orderbook from http snapshots from N configurable exchanges and updates it through
websocket depth updates. After a update the best ten asks and bids aswell as well as the spread 
are provided through a grpc server endpoint

# Lower level

IO components, ExchangeStreams, and Quote GRPC Server, are ran on seperate tokio runtimes in their own
pinned threads.

Orderbook is ran with multiplie threads. One to writing to the orderbook the others for reading the orderbook

## Components 

### ExchangeStream

Runs both http snapshot streams and websocket streams. Can handle retriggering the http snapshot stream 
but currently is not implemented in the Orderbook/ExchangeController. 

Future work: Ideally these streams are done purely on the stack but this must be verified. Correct 
sequencing of orderbook snapshots and depth updates through their timestamps

### ExchangeController

Provides a controlling interface to all exchange streams. 

Future work: 

(1) Handle more then just two exchanges
(2) Needs to handle orderbook reset and orderbook snapshot
retriggering with correct sequencing, and websocket reconnection.

### Orderbook

Handles orderbook writing and reading.  

Has 2 states:

(1) Building the orderbook with http snapshots
(2) Updating the the orderbook with ws depths and then reading the orderbook for best deals

Future work: 

(1) Required state required is rebuilding the orderbook if a ExchangeStream websocket connection fails. 
(2) Reduce dynamic memory allocations
(3) Possibly run ask and bid reader threads to their own threads and pin them to their own cores rather
then have both readers run on the same core (a threadpool also would be another lower dev cost solution here)
(4) Use a decimals or another solutiuon over floats for quantities.
(5) Use a decimals or another solutiuon over floats for price levels.
(6) Possibly more performant atomic memory ordering 

### Quote GRPC Server

Takes the spread and provides the best ten deals and asks to a grpc client

Future work:

TBD
