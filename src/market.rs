// stats dtg, dtg_last_ticker to stats
// capture book on startup
// TODO: consider moving stats variables to object-level to make them available elsewhere (like in a message to a database)
// stats btreemap
// save stats to postgresql, but only once in a while
// prune tickers and book maps occasionally
// TODO: diff_ema_roc

use std::collections::BTreeMap;
use crate::market_structs::{*};
use crate::ticker::{Ticker as TickerJson} ;
use rust_decimal::prelude::*;
use tungstenite::{Message};
use url::Url;
use crate::algorithm::*;
use crate::db::Msg;
use chrono::{Utc};

pub struct Market{

	// btree is inherently sorted, key here is coinbase sequence so most recent is always fastest
	tickers:BTreeMap<u64, TickerJson>,

	// <price, size>
	book_sell:BTreeMap<Decimal, Decimal>,

	// <price, size>
	book_buy:BTreeMap<Decimal, Decimal>,

	tx:crossbeam::channel::Sender<Msg>,

	stats:Vec<Stat>,

	trades: BTreeMap<u64 ,Trade>,

	buy_trade_unmatched: Option<Trade>,

}

impl Market{

	fn new(tx:crossbeam::channel::Sender<Msg>) -> Self{
		Self{
			tickers : BTreeMap::new(),
			book_sell : BTreeMap::new(),
			book_buy : BTreeMap::new(),
			tx : tx,
			stats: vec![],
			trades : BTreeMap::new(),
			buy_trade_unmatched : None,
		}
	}

	pub fn start(){

		println!("[start] println");

		tracing::debug!("[start]");

		// Channel for websocket thread to send to database thread
		let (tx, rx) = crossbeam::channel::unbounded();

		// This
		let mut myself:Market = Market::new(tx);

		// Start Websocket
		let mut handles = vec![];
		handles.push (std::thread::spawn( move || {
			let _ws = myself.websocket_go();
		}));

		// Start Database
		// handles.push(std::thread::spawn(move ||->(){
		// 	let db_log_url = std::env::var("COIN_TRADE_LOG_DB_URL").expect("COIN_TRADE_LOG_DB_URL not found");
		// 	if let Some(mut client) = crate::db::db_connect(&db_log_url){
		// 		crate::db::db_thread(&mut client, rx);
		// 	};
		// }));

		for h in handles {
			h.join().unwrap();
		}
	}

	fn websocket_go(&mut self){

		tracing::debug!("[websocket_go]");
		let url = std::env::var("COINBASE_URL").unwrap_or_else(|_| "wss://ws-feed.pro.coinbase.com".to_string());
		tracing::debug!("[websocket_go] url: {}", &url);
		let (mut ws, response) = tungstenite::connect(Url::parse(&url).unwrap()).unwrap();
		tracing::info!("[websocket_go] websocket connected, response: {:?}", response);

		// subscribe to coinbase socket for heartbeat and tickers
		let _ = ws.write_message(Message::Text(Market::generate_websocket_subscribe_json().to_string()));

		// parse incoming messages
		loop {
			let msg_result = ws.read_message();
			match msg_result {
				Err(e) => {
					match e {
						tungstenite::error::Error::ConnectionClosed => {
							// https://docs.rs/tungstenite/0.11.1/tungstenite/error/enum.Error.html#variant.ConnectionClosed
							// TODO: stop the loop; attempt to reopen socket
							tracing::info!("[parse_incoming_socket_blocking] socket: Error::ConnectionClosed");
							break;
						},
						_ => {
							tracing::info!("[parse_incoming_socket_blocking] socket read failed: {}", &e);
							break;
						}
					}
				},
				Ok(msg) => {
					match msg {
						tungstenite::Message::Text(t) => {

							let json_val:serde_json::Value = serde_json::from_str(&t).unwrap();

							tracing::debug!("[ws] json_val: {:?}", &json_val);

							let ws_type = json_val["type"].as_str();


							// Type
							match ws_type {

								Some("heartbeat") => {},

								Some("ticker") => {

									// json to ticker
									// tracing::debug!("[ws] ticker"); /* {}", &ws_type.unwrap());*/
									// use as_str() to remove the quotation marks
									let ticker:Option<TickerJson> = serde_json::from_value(json_val).expect("[ticker_actor] json conversion to Ticker 2 didn't work"); // unwrap_or(None);

									if let Some(obj) = ticker {

										self.process_ticker(obj);

									}
								},

								Some("l2update") => {

									// parse json
									let l2_update_opt: Option<UpdateL2> = serde_json::from_value(json_val).expect("[L2 Update] json conversion didn't work");

									// to database
									if let Some(obj) = l2_update_opt {

										// tracing::debug!("[ws] {:?}", &obj);

										self.process_book_update(obj.changes);

									}
								},

								Some("snapshot") => {

									// tracing::debug!("[ws] snapshot: {:?}", json_val);

									let snapshot_opt:Option<Snapshot> = serde_json::from_value(json_val).expect("[ws:snapshot] json conversion didn't work");
									// tracing::debug!("[ws] snapshot: {:?}", snapshot_opt);

									if snapshot_opt.is_some() {

										let snap:Snapshot = snapshot_opt.unwrap();

										for buy in &snap.bids {

											let _ = &self.book_buy.insert(buy.price.clone(), buy.size.clone());

										}

										for sell in &snap.asks {

											let _ = &self.book_sell.insert(sell.price.clone(), sell.size.clone());

										}
									}
								},
								_ => {
									tracing::debug!("[ws] unknown type: {:?}", json_val);
								},
							}
						},
						_ => {
							tracing::info!("[main] unknown socket message or something not text")
						}
					}
				}
			}
		}
	}

	fn process_ticker(&mut self, mut ticker_current:TickerJson) {

		// calculate emas
		self.update_ticker_calcs(&mut ticker_current);

		// get the max/most recent ticker before insert the new one
		// TODO: sequence numbers could come in unordered; change this to pop the max 2 after insert

		let ticker_previous:Option<TickerJson> = if self.tickers.len() > 0 {
			Some(self.tickers.last_key_value().unwrap().1.clone())
		}else {
			None
		};

		// insert the new ticker (it's the new max presumably)
		self.tickers.insert((&ticker_current).sequence, (&ticker_current).clone());

		// statistic based on this latest ticker
		let _ = &self.save_stat();

		// Get trade recommendation
		if ticker_previous.is_some() {

			// ****** Algorithm Selection *****

			let allow_losing_sale:bool = bool::from_str(std::env::var("ALLOW_LOSING_SALE").unwrap_or("false".to_owned()).as_str()).unwrap_or(false);
			let recommendation:TradeRec = if allow_losing_sale {

				// use algo that allows loss
				recommend_zero_diff_ema_trade(&ticker_current, &ticker_previous.unwrap())
			}else{

				// do not allow loss
				recommend_zero_diff_ema_trade_accept_no_loss(&ticker_current, &ticker_previous.unwrap())

			};

			// Perform trade based on recommendation
			// *****************
			//let TRADE_SIZE_TARGET:Decimal = Decimal::from_f32(0.1).unwrap();
			let trade_size_target:Decimal = Decimal::from_str(std::env::var("TRADE_SIZE_TARGET").unwrap_or("0.001".to_owned()).as_str()).unwrap();
			// **************


			match recommendation {

				TradeRec::Buy => {

					// perform_trade
					if self.buy_trade_unmatched == None {

						// BUY (only if not already in a buy status)

						// "What's the market for a buyer?"
						//let (mkt_price, mkt_size) = self.get_sell_offers_at_my_buy_price((&ticker_current).price, Decimal::from_f64(1.0).unwrap());
						let market = self.get_sell_offers_at_my_buy_price((&ticker_current).price, trade_size_target);

						// tracing::debug!("[process_ticker] buy market: {:?}", &market);

						// Create an outstanding BUY trade
						let buy_trade = Trade::new(Utc::now(), (&ticker_current).clone(), Some(market.0), Some(market.1));

						tracing::info!("[process_ticker:Buy] buy trade: {:?}", &buy_trade);

						let _ = &self.trades.insert((&buy_trade).ticker_buy.sequence, buy_trade.clone());

						// Save the unmatched buy half of the trade for match with a follow-on sell
						self.buy_trade_unmatched = Some(buy_trade.clone());

						// TODO: save the unmatched buy to the database!!!

					} /*else {
						// don't do anything if in buy status
					}*/

				},
				TradeRec::Sell => {


					// Sell
					// ...if there exists a previously unmatched buy
					if self.buy_trade_unmatched != None {

						tracing::debug!("[process_ticker:TradeRec::Sell] previous buy exists, matching...\n{:?}", &(self.buy_trade_unmatched));

						// "What's the market for a seller?"
						// Get market price and size available to buy, up to 1.0 BTC
						let market = self.get_buy_bids_at_my_sell_price((&ticker_current).price, trade_size_target);

						// New Trade: Match this new sell to the existing buy
						let matched_trade = Trade::new_with_sell(self.buy_trade_unmatched.as_ref().unwrap().to_owned(), (&ticker_current).clone(), Some(market.0), Some(market.1) );

						// insert full trade into trades buffer
						self.trades.insert((&matched_trade).ticker_buy.sequence, matched_trade.clone());

						// Send cross-thread to db via crossbeam channel
						let _ = self.tx.send(Msg::Trade(matched_trade.clone()));

						// self.print_trades();
						self.buy_trade_unmatched = None;

					} else {
						tracing::debug!("[process_ticker:TradeRec::Sell] no previous buy exists")
					}
				}
				TradeRec::Hold => {
					// do nothing
				}
			}
		}
	}














	/// I want to sell X.0 BTC at $Y.YY. What are the bids out there that would match my offer and quantity?
	/// Get the highest buy offers until my quantity is fill.
	fn get_buy_bids_at_my_sell_price(&self, _my_price:Decimal, my_size:Decimal) -> (Decimal, Decimal) {

		let mut matching_bids = vec![];
		let mut total_size = Decimal::from_u8(0).unwrap();

		let mut size_still_needed = my_size;
		let mut market_bid_cost = Decimal::from_u8(0).unwrap();

		// get all the bids to buy up to the quantity I want to sell
		// I want to sell 1.0 at $13000. So bids will probably be
		// -- 0.5 at 12999 and 0.5 at 12998
		// reverse() since we want the largest
		for (k_price, size_available) in self.book_buy.iter().rev() {

			// TODO: if book_buy size is zero, return sale not possible, don't calculate



			// Calculate how much it'll cost to buy my_size at current market condition
			//0: take 0.6 = I need 1.0, there's 0.6 available. min (1.0, 0.6)
			//1. take 0.4 = I need 0.4 more, there's 0.6 available.
			let take = std::cmp::min(size_still_needed, *size_available);
			// tracing::debug!("[get_buy_offers_at_my_sell_price] take: {}", take);
			// 0: 0.4 = 1.0 - 0.6
			size_still_needed = size_still_needed - take;
			// tracing::debug!("[get_sell_offers_at_my_buy_price] size_still_needed: {}", size_still_needed);
			// how much 'size' do we need from this price point? as much as we can get up to the amount we want
			market_bid_cost = market_bid_cost + take * k_price;










			// running list of offers to sell that'd fill my market order;
			// not totally needed except for printing
			// TODO: consider if the market is ~2x the order I need, gamble that what if someone gets there first, what's the worst case?
			matching_bids.push((k_price.clone(), size_available.clone()));
			total_size += size_available;
			if total_size >= my_size{
				break;
			}
		}

		// tracing::debug!("[get_buy_bids_at_my_sell_price] target price: {}, size: {}, available: {:?}", my_price, my_size, matching_bids);
		// tracing::debug!("[get_buy_bids_at_my_sell_price] available profit: {}, for available size: {}", market_bid_cost, (my_size - size_still_needed));
		// tracing::debug!("[get_buy_bids_at_my_sell_price] market bid is ${} less than my desired sell price", market_bid_cost -my_price);

		(market_bid_cost, (my_size - size_still_needed))
	}

	/// I want to buy X.0 BTC at $Y.YY. What are the lowest sell offers out there that would match my offer and quantity?
	/// Get the highest buy offers until my quantity is fill.
	/// Return (price,size) available to buy based on the cheapest currently offered
	fn get_sell_offers_at_my_buy_price(&self, _my_price:Decimal, my_size:Decimal) -> (Decimal, Decimal){

		let mut matching_bids = vec![];
		let mut size_still_needed = my_size;
		let mut market_cost = Decimal::from_u8(0).unwrap();
		let mut total_size = Decimal::from_u8(0).unwrap();

		// get all the bids to buy up to the quantity I want to sell
		// I want to sell 1.0 at $13000. So bids will probably be
		// -- 0.5 at 12999 and 0.5 at 12998
		// don't need to reverse for sell offers since we want the smallest
		for (k_price, size_available) in self.book_sell.iter() {

			// Calculate how much it'll cost to buy my_size at current market condition
			// Example: I want 1.0 BTC but there's only 0.5 available at this price
			// available 0.6 at $x
			// available: 0.6 at $y (only need 0.4)
			//0: take 0.6 = I need 1.0, there's 0.6 available. min (1.0, 0.6)
			//1. take 0.4 = I need 0.4 more, there's 0.6 available.
			let take = std::cmp::min(size_still_needed, *size_available);
			// tracing::debug!("[get_sell_offers_at_my_buy_price] take: {}", take);
			// 0: 0.4 = 1.0 - 0.6
			size_still_needed = size_still_needed - take;
			// tracing::debug!("[get_sell_offers_at_my_buy_price] size_still_needed: {}", size_still_needed);
			// how much 'size' do we need from this price point? as much as we can get up to the amount we want
			market_cost = market_cost + take * k_price;

			// running list of offers to sell that'd fill my market order;
			// not totally needed except for printing
			// TODO: consider if the market is ~2x the order I need, gamble that what if someone gets there first, what's the worst case?
			matching_bids.push((k_price.clone(), size_available.clone()));
			total_size += size_available;
			if total_size >= my_size{
				break;
			}
		}

		let result = (market_cost, (my_size - size_still_needed));

		// tracing::debug!("[get_sell_offers_at_my_buy_price] target price: {}, size: {}, available: {:?}", my_price, my_size, matching_bids);
		// tracing::debug!("[get_sell_offers_at_my_buy_price] total cost: {}, for available size: {}", (&result).0, (&result).1);
		// tracing::debug!("[get_sell_offers_at_my_buy_price] market cost is ${} more than my desired price", market_cost-my_price);


		result
	}


	fn process_book_update(&mut self, changes:Vec<Change>){

		// TODO: what kind of sort does this imply? probably minor but print could be out of order, FYI
		for c in changes{

			// tracing::debug!("[process_book_update], {:?}", &c);

			let size = Decimal::from_str(&c.size).unwrap();
			let size_is_zero = size == Decimal::from_u8(0).unwrap();
			let price = Decimal::from_str(&c.price).unwrap();

			if &c.side == "buy" {

				if size_is_zero {
					// if the size is zero, remove it
					let _ = &self.book_buy.remove(&price);
				} else {
					// if the size is not zero, add/replace what's in the btreemap
					let _ = &self.book_buy.insert(price, size);
				}

			} else if &c.side == "sell" {
				if size_is_zero {
					// if the size is zero, remove it
					let _ = &self.book_sell.remove(&price);
				} else {
					// if the size is not zero, add/replace what's in the btreemap
					let _ = &self.book_sell.insert(price, size);
				}
			}

			// print stats on every entry
			let _ = &self.save_stat();
		}
	}

	fn build_stat_from_latest_ticker_and_book(&self) ->Option<Stat>{

		let mut stat = Stat::new(); 

		if !&self.book_buy.is_empty() && !&self.book_sell.is_empty()  {
	
			let max_buy = self.book_buy.last_key_value().unwrap().0;
			let min_sell = self.book_sell.first_key_value().unwrap().0;
			let spread = min_sell - max_buy;

			stat.spread =  Some(spread);
			stat.min_sell =  Some(*min_sell);
			stat.max_buy =  Some(*max_buy);
		}

		if !&self.tickers.is_empty(){
			let (_,ticker_v) = self.tickers.last_key_value().unwrap();
		
			stat.dtg_last_tick =  Some(ticker_v.dtg);
			stat.seq_last_tick =  Some(ticker_v.sequence);
			stat.price =  Some(ticker_v.price);
			stat.ema1 =  ticker_v.ema1;
			stat.ema2 =  ticker_v.ema2;
			stat.diff_ema =  ticker_v.diff_ema;
			stat.diff_ema_roc =  ticker_v.diff_ema_roc;

		}

		// tracing::debug!("[latest_stat] Tickers: {}, Asks: {}, Bids: {}", &self.tickers.len(), &self.book_sell.len(), &self.book_buy.len());

		Some(stat)

	}

	/// Send the latest stat to the database
	/// Originally this printed the latest stat, potentially very inefficient, maybe send 1000 at a time?
	/// or send a ref to the stat map
	fn save_stat(&mut self) {
		if let Some(st) = self.build_stat_from_latest_ticker_and_book(){

			// save stat to memory
			self.stats.push(st);

			// save stat to the database
			if self.stats.len() > 999 {
				// clean out the stats vector and push to the database
				let _ = self.tx.send(Msg::StatVector(self.stats.drain(..).collect()));
			};
		};
	}

	fn generate_websocket_subscribe_json() -> serde_json::Value {
		let cb_sub = Subscribe{
			typ:"subscribe".to_owned(),
			product_ids:vec!["BTC-USD".to_owned()],
			// channels:vec!["ticker".to_owned(), "level2".to_owned(), "user".to_owned()]
			// channels:vec!["ticker".to_owned(), "level2".to_owned()]
			channels:vec!["ticker".to_owned()]
		};
		let j : serde_json::Value = serde_json::to_value(&cb_sub).expect("[json_ws_subscribe] json serialize failed");
		j.to_owned()
	
	}

	// TODO: not so "functional", causes an effect
	fn update_ticker_calcs(&self, t: &mut TickerJson) {

		t.ema1 = self.compute_moving_average(5, t.price);
		t.ema2 = self.compute_moving_average(20, t.price);
		if t.ema1.is_some() && t.ema2.is_some() {
			t.diff_ema = Some(t.ema1.unwrap() - t.ema2.unwrap());
			t.diff_price_ema1 = Some(t.price - t.ema1.unwrap());
			t.diff_price_ema2 = Some(t.price - t.ema2.unwrap());
		}

		// tracing::debug("[update_ticker_calcs] {:?}", &t);

	}

	///
	/// get_ema20()
	///
	/// Calculate the 20 "day" exponential moving average
	/// n is the number of periods to include
	/// alpha (and 1 minus alpha) are the weights to give the current price and the previous average
	/// Sort the db descending, take the most recent n entries
	/// Do this before inserting the current price
	fn compute_moving_average(&self, n_days: usize, curr_price: Decimal) -> Option<Decimal> {

		// Exponential Moving Average variables
		// let n = 20;
		let smoothing_factor = Decimal::from_f64(2.0).unwrap();
		let alpha = smoothing_factor / Decimal::from_f64(1.0 + (n_days as f64)).unwrap();

		// sorted descending
		// Gets an iterator over the values of the map, in order by key.
		// https://doc.rust-lang.org/std/collections/struct.BTreeMap.html#method.values
		let v: Vec<&TickerJson> = self.tickers.values().rev().collect();
		// tracing::debug!("[compute_moving_average] \n{:?}", &v);

		// get a slice of the hashmap (for 5 day ema, len() could be 3, but only want 2, at index 1, 2 with 0 being current)
		let len_max = std::cmp::min(v.len(), n_days);
		match len_max {
			len_max if len_max > 1 => {

				let ema20_sum: Decimal = v[0..len_max].iter().fold(Decimal::from_u8(0).unwrap(), |sum, i| sum + i.price);

				let avg_of_n = ema20_sum / Decimal::from_usize(len_max).unwrap();

				// EMA = price (1-alpha) + average_previous_n (alpha)
				// TODO: confirm this shouldn't be the other way around, giving higher weight to the average than the current
				let ema_decimal = curr_price * (Decimal::from_usize(1).unwrap() - alpha) + avg_of_n * (alpha);

				Some(ema_decimal)
			},
			len_max if len_max == 1 => {
				None
			},
			_ => {
				// not possible
				None
			}
		}
	}
}
