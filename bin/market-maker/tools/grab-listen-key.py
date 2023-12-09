import ssl
from websocket import create_connection

import requests

KEY = "sZERzNRbThX0dyhUJM1fATjrcFFEvpdPf2HGWP38ZWnZsPbyeUBKtJfyfDeXuUEN"
url = 'https://api.binance.com/api/v1/userDataStream'
listen_key = requests.post(url, headers={'X-MBX-APIKEY': KEY})['listenKey']
connection = create_connection('wss://stream.binance.com:9443/ws/{}'.format(KEY),
                               sslopt={'cert_reqs': ssl.CERT_NONE})


class exchange:
    def __init__(self, uri):
        self.uri = uri
        self.ws = websockets.connect(uri)

    # TODO: Move connecting to the websocket its own method and simplify write_ws
    async def connect(self):
        try:
            async with websockets.connect(uri) as websocket:
                self.open = websocket
        except (websockets.ConnectionClosedOK, websockets.ConnectionClosedError, websockets.ConnectionClosed, websockets.InvalidState, websockets.PayloadTooBig, websockets.ProtocolError) as e:
                print(e)

    async def read_ws(self):
        while True:
            try:
                msg = await self.open.recv()
            except (websockets.ConnectionClosedOK, websockets.ConnectionClosedError, websockets.ConnectionClosed, websockets.InvalidState, websockets.PayloadTooBig, websockets.ProtocolError) as e:
                print(e)
                self.open = await websockets.connect(self.uri)  # reconnect to websocket
                msg = await self.open.send(self.subscription)
                continue
            except Exception as e:
                print(e)
                quit()
            else:
                python_object = orjson.loads(msg)
                # TODO: May be a faster way to check for contents
                # TODO: Make this asset agnostic and configurable by flag input
                if 'contents' in python_object:
                    #if 'markets' in python_object['contents']:
                    print(python_object['contents'])
                        #print(python_object['contents']['markets']['CELO-USD'])

    async def write_ws(self, payload):
        retry = 0
        connected = False
        self.subscription = payload
        while retry < 3:
            print("send attempt {}", retry)
            try:
                match connected:
                    case True:
                        print("connected")
                        break
                    case False:
                        async with self.ws as websocket:
                            self.open = websocket
                            msg = await websocket.send(payload)
                            connected = True
            except (websockets.ConnectionClosedOK, websockets.ConnectionClosedError, websockets.ConnectionClosed, websockets.InvalidState, websockets.PayloadTooBig, websockets.ProtocolError) as e:
                quit()
            except Exception as e:
                print(e)
            else:
                print("received from write: {}", msg)
                if msg == None:
                    print("failed to subscribe")
                    retry +=1
                    continue
        if retry == 3 and connected == False:
            quit()

