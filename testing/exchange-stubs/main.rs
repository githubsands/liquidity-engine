async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut exchange_server = ExchangeServer::new(name, ip_address, port);
    exchange_server.run().await;
    Ok(())
}
