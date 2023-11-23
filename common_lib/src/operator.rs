//! operator.rs

use std::fmt::Debug;
use crossbeam_channel::{Sender, unbounded};
use crate::cb_ticker::Ticker;
use crate::heartbeat::start_heartbeat;

#[derive(Debug)]
pub enum Msg {
    // Post(T),
    Post(Ticker),
    // PostAndLog(T),
    Ping,
    Pong,
    Start,
    Stop,
}

/// spawn a thread to listen for messages; return a way to send it crossbeam messages
pub fn run(tx_db: Sender<Msg>) -> Sender<Msg> {
    let (tx,rx) = unbounded();
    let tx2 = tx.clone();
    std::thread::spawn(move ||{
        loop{
            match rx.recv(){
                Ok(message)=> process_message(message, tx_db.clone()),
                Err(e)=> tracing::debug!("[operator] error {:?}", &e),
            }
        }
    });
    let tx3 = tx2.clone();
    let _h = start_heartbeat(tx3);
    tx
}

fn process_message(message:Msg, tx_db: Sender<Msg>){
    match message{
        Msg::Ping => tracing::debug!("[operator] PING"),
        Msg::Post(msg)=> tx_db.send(Msg::Post(msg)).unwrap(),
        _ => tracing::debug!("[operator] {:?} UNKNOWN ", &message)
    }
}

