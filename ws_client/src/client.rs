//! client
//!
//! websocket server
//!

use common_lib::{DbMsg};
use std::error::Error;
use std::thread::JoinHandle;
use crossbeam::channel::Sender;
use tungstenite::{connect};
use url::Url;
use crate::{ws_alpaca, ws_coinbase};

const ALPACA_CRYPTOCURRENCY_URL: &str = "wss://stream.data.alpaca.markets/v1beta3/crypto/us";
// const COINBASE_URL: &str = "wss://ws_client-feed.pro.coinbase.com";
const COINBASE_URL: &str = "wss://ws-feed.exchange.coinbase.com";

#[derive(Debug)]
pub enum ConnectSource {
    Alpaca,
    Coinbase,
}

/// Start a new thread listening to the coinbase websocket
pub fn run(source: ConnectSource, tx_db: Sender<DbMsg>) -> JoinHandle<()> {
    tracing::debug!("[run] spawning websocket...");
    std::thread::spawn(move || {
        let _ws = ws_connect(source, tx_db);
    })
}

/// connect to alpaca or coinbase websocket
pub fn ws_connect(source: ConnectSource, tx_db: Sender<DbMsg>) -> Result<(), Box<dyn Error>> {
    // https://doc.rust-lang.org/book/ch09-02-recoverable-errors-with-result.html
    let url = match source {
        ConnectSource::Alpaca => std::env::var("ALPACA_CRYPTOCURRENCY_URL").unwrap_or_else(|_| ALPACA_CRYPTOCURRENCY_URL.to_string()),
        ConnectSource::Coinbase => std::env::var("COINBASE_URL").unwrap_or_else(|_| COINBASE_URL.to_string()),
    };
    tracing::debug!("[ws_connect] url: {}", &url);
    #[allow(unused_mut)]
    let (mut socket, response) = connect(Url::parse(&url)?)?;

    tracing::debug!("[ws_connect] {source:?} response: {response:?}");

    match source {
        ConnectSource::Alpaca => {
            ws_alpaca::parse(socket, tx_db);
        },
        ConnectSource::Coinbase => {
            ws_coinbase::parse(socket, tx_db);
        }
    }

    Ok(())
}