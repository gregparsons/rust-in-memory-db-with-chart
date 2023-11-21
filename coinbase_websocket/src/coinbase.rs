//! coinbase.rs

use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Coinbase{
	Subscriptions(Subscriptions),
	Heartbeat,
	Ticker(Ticker),
	// L2Update,
	// Snapshot,

}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Channel{
	name:String,
	product_ids:Vec<String>
}

///    "{\"type\":\"subscriptions\",\"channels\":[{\"name\":\"ticker\",\"product_ids\":[\"BTC-USD\"]}]}"
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", tag = "Subscription")]
pub struct Subscriptions {
	channels:Vec<Channel>,
}

/// "{\"type\":\"ticker\",\"sequence\":68163111365,\"product_id\":\"BTC-USD\",\"price\":\"36685.01\",\"open_24h\":\"35799.36\",\"volume_24h\":\"29062.82961427\",\"low_24h\":\"35555.16\",\"high_24h\":\"37999\",\"volume_30d\":\"414208.58541546\",\"best_bid\":\"36685.01\",\"best_bid_size\":\"0.06260238\",\"best_ask\":\"36688.09\",\"best_ask_size\":\"0.08893378\",\"side\":\"sell\",\"time\":\"2023-11-09T22:16:05.023729Z\",\"trade_id\":576024484,\"last_size\":\"0.00009645\"}"
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Ticker{
	#[serde(rename="time")]
	dtg:DateTime<Utc>,
	product_id:ProductId,
	price:BigDecimal,
}

#[derive(Debug, Deserialize)]
pub enum ProductId {
	#[serde(rename="BTC-USD")]
	BtcUsd,
}