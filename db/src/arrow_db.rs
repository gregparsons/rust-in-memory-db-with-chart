//! db.rs
//!
//! thread/message receiver for database operations; calls into event_log.rs for the in-memory
//! data structures.
//!

use common_lib::heartbeat::start_heartbeat;
use crossbeam_channel::{unbounded, Sender};
use std::sync::Arc;
use tokio::runtime::Handle;
use common_lib::{UniversalError, DbMsg};
use common_lib::cb_ticker::Datasource;
use crate::calculation::refresh_calculations;
use crate::event_book::EventBook;

pub const BOOK_NAME_COINBASE:&str="coinbase";
pub const LIMIT_RETURN_SIZE:usize = 1000;

/// spawn a thread to listen for messages; return the channel to communicate to this thread
pub fn run(tr: Handle) -> Sender<DbMsg> {
    tracing::debug!("[run]");
    let (tx, rx) = unbounded();
    let event_book = Arc::new(EventBook::new());
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        tracing::debug!("[run] inside thread::spawn 0");
        let _ = start_heartbeat(tx2);
        loop {
            // tracing::debug!("[run] inside loop");
            match rx.recv() {
                Ok(message) => {
                    // tracing::debug!("[run] message: {:?}", &message);
                    let evt_book = event_book.clone();

                    // new thread to prevent processing blocking the websocket
                    if let Err(e) = receive(message, &evt_book, tr.clone()){
                        tracing::info!("[run] message error: {:?}", e);
                    }
                }
                Err(e) => tracing::error!("[db] error {:?}", &e),
            }
        }
    });
    tx
}

fn receive(message: DbMsg, evt_book: &EventBook, tr: Handle) -> Result<(), UniversalError>  {

    // tracing::debug!("[db::receive] msg:{:?}", &message);

    match message {

        DbMsg::Ping => {
            tracing::debug!("[db] PING");
            Ok(())
        },

        DbMsg::Insert(ticker_src, ticker) => {

            tracing::debug!("[receive] insert ({ticker_src:?}): {:?}", &ticker);

            match ticker_src {
                Datasource::Coinbase => {
                    let _ = evt_book.push_log(Datasource::Coinbase, &ticker);
                    if let Err(e) = refresh_calculations(Datasource::Coinbase, evt_book, ticker.symbol) {
                        tracing::error!("[process_message] refresh_calculations error: {:?}", &e);
                    }
                    Ok(())
                }
                Datasource::Alpaca => {
                    let _ = evt_book.push_log(Datasource::Alpaca, &ticker);
                    // if let Err(e) = refresh_calculations(Datasource::Alpaca, evt_book, ticker.symbol) {
                    //     tracing::error!("[process_message] refresh_calculations error: {:?}", &e);
                    // }
                    Ok(())
                }
            }
        }

        DbMsg::RqstChartMulti {sender, symbol} => {
            let evt_book_read_lock = evt_book.book.read().unwrap();

            let mut chart = vec!();

            let mut chart_cb = match evt_book_read_lock.get(&Datasource::Coinbase){
                Some(evt_log)=>{
                    tr.block_on(async{
                        evt_log.chart_since(Datasource::Coinbase, &symbol, None, LIMIT_RETURN_SIZE).await
                    })
                },
                None=> Err(UniversalError::DbError("RqstChartMulti".to_string())),
            }?;

            let mut chart_alp = match evt_book_read_lock.get(&Datasource::Alpaca){
                Some(evt_log)=>{
                    tr.block_on(async{
                        evt_log.chart_since(Datasource::Alpaca, &symbol, None, LIMIT_RETURN_SIZE).await
                    })
                },
                None=> Err(UniversalError::DbError("RqstChartMulti".to_string())),
            }?;

            chart.append(&mut chart_cb);
            chart.append(&mut chart_alp);

            // tracing::info!("[returning chart] {:?}", &chart);
            match sender.send(chart) {
                Err(e)=> {
                    tracing::error!("[Msg::RequestChartRust] {:?}", &e);
                    Err(UniversalError::SendError)
                },
                _ => Ok(()),
            }
        }

        DbMsg::RqstChartSince {sender, symbol, since} => {

            let mut chart = vec!();
            let evt_book_read_lock = evt_book.book.read().unwrap();
            let mut chart_cb = match evt_book_read_lock.get(&Datasource::Coinbase){
                Some(evt_log)=>{
                    tr.block_on(async{
                        evt_log.chart_since(Datasource::Coinbase, &symbol, Some(since), LIMIT_RETURN_SIZE).await
                    })
                },
                None => {
                    tracing::info!("[DbMsg::RqstChartSince] no coinbase records found");
                    // Err(UniversalError::DbError("RqstChartMultiSince".to_string()))
                    Ok(vec!())
                }
            }?;

            let mut chart_alpaca = match evt_book_read_lock.get(&Datasource::Alpaca){
                Some(evt_log)=>{
                    tr.block_on(async{
                        evt_log.chart_since(Datasource::Alpaca, &symbol, Some(since), LIMIT_RETURN_SIZE).await
                    })
                },
                None => {
                    tracing::info!("[DbMsg::RqstChartSince] no alpaca records found");
                    // Err(UniversalError::DbError("RqstChartMultiSince".to_string()))
                    Ok(vec!())
                }
            }?;

            chart.append(&mut chart_cb);
            chart.append(&mut chart_alpaca);

            // tracing::info!("[returning chart] {:?}", &chart);
            match sender.send(chart) {
                Err(e)=> {
                    tracing::error!("[Msg::RequestChartRust] {:?}", &e);
                    Err(UniversalError::SendError)
                },
                _ => Ok(()),
            }
        }

        // send a DataFrame back with 'select * from ..."
        DbMsg::RqstRaw {ticker_source, sender}=>{
            let evt_book_read_lock = evt_book.book.read().unwrap();
            let df = match evt_book_read_lock.get(&ticker_source){
                Some(evt_log)=>{
                    let df_result = tr.block_on(async{
                        evt_log.query_sql_for_chart().await
                    });
                    match df_result{
                        Ok(df)=>Ok(df),
                        Err(e)=>{
                            tracing::error!("{:?}", e);
                            Err(UniversalError::DbError("RqstRaw".to_string()))
                        }
                    }
                },
                None=> Err(UniversalError::DbError("RqstRaw".to_string())),
            }?;

            match sender.send(df) {
                Err(_e)=> Err(UniversalError::SendError),
                _ => Ok(()),
            }
        },

        _ => {
            tracing::debug!("[db] {:?} UNKNOWN ", &message);
            Err(UniversalError::NoMessageMatch)
        },
    }
}







#[cfg(test)]
mod tests{

    use common_lib::{ChartAsJson};

    /// confirm the Chart object resolves from a known correct json string
    #[test]
    fn json_test() {

        let json = r#"
        {
            "columns":["2023-08-14", "2023-08-15", "2023-08-16", "2023-08-17", "2023-08-18", "2023-08-21", "2023-08-22", "2023-08-23", "2023-08-24", "2023-08-25", "2023-08-28", "2023-08-29", "2023-08-30", "2023-08-31", "2023-09-01", "2023-09-05", "2023-09-06", "2023-09-07", "2023-09-08", "2023-09-11", "2023-09-12", "2023-09-13", "2023-09-14", "2023-09-15", "2023-09-18", "2023-09-19", "2023-09-20", "2023-09-21", "2023-09-22", "2023-09-25", "2023-09-26", "2023-09-27", "2023-09-28", "2023-09-29", "2023-10-02", "2023-10-03", "2023-10-04", "2023-10-05", "2023-10-06", "2023-10-09", "2023-10-10", "2023-10-11", "2023-10-16", "2023-10-17", "2023-10-18", "2023-10-19", "2023-10-20", "2023-10-23", "2023-10-24", "2023-10-25", "2023-10-26", "2023-10-27", "2023-10-30", "2023-10-31", "2023-11-03", "2023-11-06", "2023-11-07", "2023-11-08", "2023-11-09", "2023-11-10", "2023-11-13", "2023-11-14", "2023-11-15", "2023-11-17", "2023-11-20", "2023-11-27", "2023-11-28", "2023-11-29", "2023-11-30", "2023-12-01"],
            "chart_data":[
                {
                    "key" : "aapl",
                    "val" : [0.0177, 0.0650, -0.0260, -0.0408, -0.0055, -0.0079, 0.0073, 0.2684, 0.0, -0.0078, 0.3213, 0.5855, 0.3809, 0.5024, 0.4700, 0.2817, 0.0, -24.2500, -0.2045, 0.1322, -0.3000, 0.0, -0.6600, 0.0, 0.0283, -0.1167, -7.1800, -0.3925, -0.2133, 0.2886, -7.1500, 0.4960, 0.1594, 0.2790, 0.1800, 0.1050, -0.4800, 0.1321, 0.1048, 0.2733, -0.3864, 0.0, -0.4240, 0.0, 0.0, -0.2371, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.2478, 0.7418, 0.2986, 0.3938, -0.4300, 0.3300, -2.9533, 1.3705, 0.1983, 0.3046, 16.9000, 0.3982, -0.6790, -3.1500, -0.7750, 0.2633]}, {"key" : "amzn", "val" : [0.0372, -0.0300, -0.0537, -0.0142, -0.0062, 0.0027, -0.0205, -0.0692, -0.4325, 0.4794, 0.1750, 0.3212, 0.2354, 0.5859, -0.4818, 0.0310, 0.1150, 0.1667, 0.2677, 0.2070, 0.0400, 0.4937, 0.0, 0.0, 0.3656, -5.2150, -0.1867, -0.9789, -0.0371, 0.3600, -0.3050, -1.6250, 0.3800, 1.3862, -0.0506, 0.0, -11.9900, -0.0600, 0.1829, -0.6379, 0.2578, 0.0, -0.0015, -14.5000, -1.6014, 0.5000, 0.0, 0.0, 0.0, -59.0400, 0.0, 3.0400, 0.4917, 0.2750, 8.7100, -0.2900, 0.2327, -0.2364, -0.6225, 0.3926, 0.2190, 1.7533, -27.0400, -0.2117, 2.7100, -0.1505, 0.0414, 0.9308, 1.6255, 0.0579]}, {"key" : "bbai", "val" : [-0.0100, 0.0, -0.0126, -0.0100, -0.0100, -0.0300, 0.0050, -0.0100, -0.0067, -0.1950, 0.0100, 0.0200, -0.0138, -0.0450, 0.0180, -0.0233, 0.0100, -0.9000, 0.0350, 0.0033, 0.0433, -0.0400, -0.0400, 0.0, 0.0050, 0.0, -0.1633, -0.0200, -0.0050, -0.0150, -0.0300, -0.0200, -0.0050, -0.0267, -0.5267, -0.0200, 0.0133, 0.0150, -0.0200, -0.0050, 0.0, -0.0900, -0.0050, 0.0600, -0.0150, 0.0033, -0.0400, -0.0267, 0.0050, 0.0, -0.0500, -0.0200, 0.0, -0.0133, 0.0, -0.0367, -0.0838, -0.0125, 0.0100, -0.0800, 0.0963, 0.0500, -0.0020, 0.0, 0.0, 0.0100, -0.3000, -0.0067, -0.0300, 0.0025]}, {"key" : "dis", "val" : [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1877, 0.0825, 0.4193, 0.0929, -2.4233, -0.0408, 0.0, 0.0, -0.1900, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -0.9900, 0.0250, 0.0, 0.1389, 0.2400, -0.4067, -0.3807, 0.1000, 0.1275, 0.0600, 0.0, 0.0, 0.0, 0.0, 0.0, 0.6700, -0.6650, 0.2100, -0.2889, 2.1167, 0.7800, -0.0548, 0.4453, 0.5746, -0.5840, 0.0300, -1.1167, -3.5920, -0.3494, -0.3140, -1.0000]}, {"key" : "nio", "val" : [0.0053, -0.2600, -0.0392, -0.0076, -0.0348, -0.0050, -0.0113, -0.0214, -0.0675, 0.3100, 0.0500, 0.1420, -0.0767, -0.3189, 0.2189, -0.0175, 0.0064, -0.5100, -0.0867, 0.0207, -0.0971, -0.9750, 0.1200, 0.0, -0.0117, -0.5140, -0.0650, -0.0967, 0.1583, 0.1520, -0.0118, 0.0050, 0.1185, -0.0814, -0.3000, -0.3038, 0.0004, -0.0050, 0.0050, -0.0220, 0.0753, 0.0, 0.0050, 0.0000, -0.0200, -0.9050, -0.0800, -0.2133, 0.0352, -0.4700, 0.0086, 0.0700, -0.0280, -0.1375, 0.0100, -0.2120, 0.0030, -0.0060, 0.0100, 0.0, 0.0, 0.0, 0.0360, -0.0180, 0.2800, -0.0150, 0.0133, 0.0650, 0.0, 0.0]}, {"key" : "pacw", "val" : [-0.0100, -0.2900, -0.0214, -0.0067, -0.0200, -0.0108, -0.0200, -0.0358, 0.0000, 0.0600, 0.0233, -0.0183, 0.0000, -0.1820, 0.0164, 0.0, -0.0033, -0.2100, 0.0300, -0.0040, 0.0020, -0.0420, 0.0000, 0.0, -0.1660, -0.2533, 0.0000, -0.0038, -0.0022, 0.0518, 0.1600, -0.8067, -0.0158, 0.1157, -0.1025, -0.6933, 0.0150, -0.1450, 0.1375, -0.0214, 0.0586, -0.0200, -0.0145, 2.3000, -0.0983, -0.0100, 1.5600, -0.1300, -0.2260, -1.6000, -0.0188, 0.0, 0.0, 0.1000, -0.0425, 0.0800, -0.2400, -0.3040, 0.0450, 0.0, 0.0, 0.1641, -0.0500, -0.0060, 0.5000, 0.0014, -0.0320, -0.2480, -0.1667, 0.0]}, {"key" : "rivn", "val" : [0.0105, 0.0050, -0.0070, -0.0136, -0.0155, -0.0065, -0.0241, -0.0236, -0.0867, 0.2777, 0.3440, 0.3618, 0.2373, 0.1556, 0.1280, 0.0600, 0.0, -0.5000, -0.4100, 0.0078, -0.7786, -0.0327, 0.0500, 0.0, -0.0550, -2.6933, -1.4900, -0.0144, 0.1340, 0.0, 0.0392, 0.2230, -0.0329, 0.1279, 0.2682, -1.7800, 0.1208, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]}, {"key" : "t", "val" : [-0.0155, -0.1600, -0.0103, -0.0096, -0.0139, -0.0057, -0.0174, -0.0167, 0.0178, -0.0050, 0.0733, 0.0300, -0.0400, -0.0138, -0.3625, -0.0220, 0.0342, 0.0350, -0.3400, 0.0000, 0.0227, -0.0186, 0.0, 0.8800, 0.0180, -0.0145, 0.0227, -0.0114, -0.2233, -0.0040, -0.5200, -0.0108, 0.0217, -0.0080, -0.6067, -0.0033, -0.0127, 0.0200, -0.2000, 0.0300, 0.0340, 0.0, 0.0280, -0.1400, -0.2580, 0.0827, 0.3000, -0.4400, 0.0900, 0.0700, -0.0550, -0.1267, 0.2080, 0.0429, -0.0033, 0.0, -0.1633, -0.1000, -0.0887, 0.0036, -0.0600, 0.0555, -0.0620, -0.0025, -0.0200, 0.0117, -0.3550, 0.0083, 0.0900, 0.0300]}, {"key" : "tsla", "val" : [0.0295, -0.0733, -0.0345, -0.0994, -0.0168, 0.0746, -0.0429, 0.1759, 0.4217, -0.0909, 0.7257, 0.5605, 0.1517, 0.2135, 1.1150, 0.5218, 0.0, -36.8100, -0.6758, 0.3050, 0.5300, 0.4392, 42.5700, 0.0, -0.0820, 0.3977, 0.0800, 0.5473, -0.1233, -4.8800, 0.6380, -1.2037, 0.4044, -0.2179, 0.6480, 0.0, 0.9333, -0.0264, 0.0938, 0.0989, -0.8228, 0.0, -0.0511, 0.0, -5.1743, 3.2000, 0.0700, -12.7400, -0.0604, -26.5500, 0.0, 0.0, 0.0, 0.0, 1.8229, 2.4900, -1.8033, 0.8751, 0.0, 0.0, 0.1635, 1.1853, 0.1947, -2.1267, 1.1000, -0.0416, 0.5980, -2.0569, -2.6856, -2.7100]}, {"key" : "uber", "val" : [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.3375, -0.4500, 0.2609, 0.0527, 0.0400, 0.0467, 1.7600, 0.0, -0.2629, 0.0693, 0.0378, -3.6100, 0.2364, 0.6833, -0.7425, 0.0070, 0.1110, -0.0671, -0.5275, 0.1317, 0.0019, -0.5167, 0.1525, 0.0633, 0.3560, -0.2100, 0.0760, 0.0100, -0.0310, -0.1325, 0.0600, 0.1250, 0.6313, -18.4000, -0.0250, 0.3978, 0.8613, 0.0781, 7.9883, 0.1757, 0.1371, 0.3000, 0.2794, 0.2950, -0.1511, 0.4220, -0.0929, 0.0475, -5.2100, -0.0650, 0.1488, 0.1058, -0.0513, 0.0786]
                }
            ]
        }
        "#;

        match serde_json::from_str::<ChartAsJson>(json) {
            Ok(json) => {
                println!("[json_test] json success: {:?}", &json);
            },
            Err(e) => {
                println!("[json_test] serde conversion error: {:?}", &e);
            },
        };
        assert!(serde_json::from_str::<ChartAsJson>(json).is_ok());

    }

}
