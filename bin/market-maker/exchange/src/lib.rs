// TODO: may be wise to share module that the orderbook-quoter-server uses
struct Exchange {
    uri: String,
    order_sink: Option<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>,
    order_stream: Option<SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>>,
}

impl Exchange {
    fn new(uri: String) -> Exchange {
        Exchange {
            uri: uri,
            order_sink: None,
            order_stream: None,
        }
    }
    async fn boot(&mut self) {
        let (websocket_stream, _) = connect_async("ws://".to_owned() + self.uri.as_str())
            .await
            .expect("Failed to connect");
        let (sink, stream) = websocket_stream.split();
        self.order_sink = Some(sink);
        self.order_stream = Some(stream);
    }
    async fn receive_order_stream(&mut self) {
        while let Some(order_feedback) = self.order_stream.as_mut().unwrap().next().await {}
    }
    async fn send_order_transaction(
        &mut self,
    ) -> Result<(), tokio_tungstenite::tungstenite::Error> {
        self.order_sink
            .as_mut()
            .unwrap()
            .send(Message::Text("wip".to_string()));
        Ok(())
    }
}
