use seaquel_server::{build_router, AppState};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    // Default to loopback-only. In the production tenant container the Node
    // process (server.js) is the only public-facing thing; this service is
    // reached exclusively via Node's proxy at 127.0.0.1:8788. Override with
    // BIND_ADDR=0.0.0.0:PORT only for standalone dev (`cargo run -p seaquel-server`).
    let addr: SocketAddr = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8788".to_string())
        .parse()
        .expect("BIND_ADDR must be a valid socket address");

    let app = build_router(AppState::default());

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");

    log::info!("seaquel-server listening on {}", addr);
    axum::serve(listener, app).await.expect("server error");
}
