//! main/chat_main

/*

docker run -d \
--rm \
--name market_watcher \
-e RUST_LOG="info" \
-e COINBASE_URL=wss://ws-feed.pro.coinbase.rs.com \
-e COIN_TRADE_LOG_DB_URL=postgres://postgres:PASSWORD@10.1.1.205:54320/coin_test \
-e COINBASE_URL=wss://ws-feed-public.sandbox.pro.coinbase.rs.com \
-e COIN_TRADE_LOG_DB_URL=postgres://postgres:PASSWORD@10.1.1.205:54320/coin_test \
-e TRADE_SIZE_TARGET=0.01 \
market_watcher:latest

*/

use std::error::Error;
use std::future::Future;
use std::time::Duration;
use chrono::{DateTime, Utc};
use tokio::sync::oneshot;
use arrow_lib::arrow_db;
use coinbase_websocket::ws_inbound;
use common_lib::{ChartDataset, KitchenSinkError, Msg};
use common_lib::init::init;
use visual::http_server;

fn main() {

    // general logging stuff I always do
    init(env!("CARGO_MANIFEST_DIR"));

    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        // .worker_threads(7)
        .on_thread_start(|| {})
        .on_thread_stop(|| {})
        .thread_name("actix")
        .enable_all()
        .build()
        .expect("Tokio runtime didn't start");

    // database thread
    let tx_db = arrow_db::run(tokio_runtime.handle().clone());

    // websocket thread
    let h = ws_inbound::run(tx_db.clone());

    // outbound websocket
    let (server_tx, server_rx) = crossbeam_channel::unbounded::<ws_broadcast::command::Cmd>();
    let h1 = std::thread::spawn(move || {

        std::thread::spawn(|| {
            let mut server = ws_broadcast::server::Server::new();
            server.run(server_rx);
        });
    });


    let tx_db2 = tx_db.clone();
    tokio_runtime.block_on(async {


        let tx_db3 = tx_db2.clone();
        tokio::spawn(async move {
            // todo: outbound websocket test; move somewhere else
            // todo: get the date of the last chart data sent and send
            for i in 0..100 {
                let since = DateTime::<Utc>::from(DateTime::parse_from_rfc3339("2023-12-24T04:08:00-00:00").unwrap());
                match request_chart_multi_data_since(tx_db3.clone(), since).await{
                    Ok(chart_vec) => {


                        if let Ok(json_str) = serde_json::to_string::<Vec<ChartDataset>>(&chart_vec){

                            tracing::debug!("[main] chart: {}", &json_str);

                            // server_tx.send(ws_broadcast::command::Cmd::Broadcast(format!("hello from test: {i}"))).unwrap();
                            server_tx.send(ws_broadcast::command::Cmd::Broadcast(json_str)).unwrap();

                        }

                    },
                    Err(_) => {}
                }

                std::thread::sleep(Duration::from_secs(1));
            }

        });









        tracing::info!("[main] web server starting on http://127.0.0.1:8080");
        match http_server::run(tx_db2).await{
            Ok(_) => tracing::debug!("[main] web server started on http://127.0.0.1:8080"),
            Err(e) => tracing::debug!("[main] web server not started: {:?}", &e),
        }
    });

    h.join().unwrap();
    // loop {};
}


/// duplicated from visual
async fn request_chart_multi_data_since(tx_db: crossbeam_channel::Sender<Msg>, since: DateTime<Utc>) -> Result<Vec<ChartDataset>, Box<dyn Error>> {
    let (sender, rx) = oneshot::channel();
    match tx_db.send(Msg::RqstChartMultiSince {sender, since}) {
        Ok(_)=> {
            let chart = rx.await?;
            Ok(chart)
        },
        Err(_)=> Err(Box::new(KitchenSinkError::SendError))
    }
}