//! Multi-room WebSocket chat server.
//!
//! Open `http://localhost:8080/` in browser to test.

use actix_files::NamedFile;
use actix_web::{web, Error, HttpRequest, HttpResponse, Responder};
use tokio::task::spawn_local;
use crate::chat_handler;
use crate::chat_server::ChatServerHandle;

// pub use self::server::{ChatServer, ChatServerHandle};

/// Connection ID.
pub type ConnId = usize;

/// Room ID.
pub type RoomId = String;

/// Message sent to a room/client.
pub type Msg = String;

pub async fn get_chat() -> impl Responder {
    NamedFile::open_async("visual/static/templates/chat.html").await.unwrap()
}



/// Handshake and start WebSocket handler with heartbeats.
pub async fn chat_ws(req: HttpRequest, stream: web::Payload, chat_server: web::Data<ChatServerHandle>) -> Result<HttpResponse, Error> {
    let (res, session, msg_stream) = actix_ws::handle(&req, stream)?;
    // spawn websocket handler (and don't await it) so that the response is returned immediately
    spawn_local(chat_handler::chat_ws(
        (**chat_server).clone(),
        session,
        msg_stream,
    ));
    Ok(res)
}

/*
// note that the `actix` based WebSocket handling would NOT work under `tokio::main`
#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));
    log::info!("starting HTTP server at http://localhost:8080");

    let (chat_server, server_tx) = ChatServer::new();

    let chat_server = spawn(chat_server.run());

    let http_server = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(server_tx.clone()))

            // WebSocket UI HTML file
            .service(web::resource("/").to(index))

            // websocket routes
            .service(web::resource("/ws").route(web::get().to(chat_ws)))

            // enable logger
            .wrap(middleware::Logger::default())

    })
    .workers(2)
    .bind(("127.0.0.1", 8080))?
    .run();

    try_join!(http_server, async move { chat_server.await.unwrap() })?;

    Ok(())
}

 */
