use exchange_stubs::ExchangeServer;
use std::env;

use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
    let ws_port = env::var("WS_PORT")?.parse::<u16>()?;
    let http_port = env::var("HTTP_PORT")?.parse::<u16>()?;
    let (mut exchange_server, _) =
        ExchangeServer::new("test".to_string(), ws_port, http_port).unwrap();
    exchange_server.run_http_server().await?;
    exchange_server.run_websocket().await?;
    Ok(())
}
