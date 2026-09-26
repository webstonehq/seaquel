//! The `seaquel-server` binary.
//!
//! # Security: loopback only
//!
//! This server has no authentication of its own. It trusts whoever reaches
//! it:
//!
//! - `POST /rpc` takes the user from the `X-Seaquel-User` header and opens
//!   that user's `DATA_DIR/users/<id>/meta.db`. Anyone who can send a request
//!   here can name any user and read or write their saved connections,
//!   queries and history.
//! - `/api/db/*` keeps every user's database connections in one map; tenant
//!   isolation is the `userId:` prefix that Node checks and strips.
//!
//! So the Node process (`server.js`) must be the only thing that can reach
//! it. Node checks the session, sets `X-Seaquel-User` itself and drops any
//! copy the browser sent. The default bind is `127.0.0.1:8788`, and
//! `server.js` passes that explicitly. Never bind it to a public or shared
//! interface (`BIND_ADDR=0.0.0.0:…`) in a deployment, and never publish its
//! port from a container. The server refuses to start on a non-loopback
//! `BIND_ADDR` unless `SEAQUEL_ALLOW_NON_LOOPBACK=1`, which exists only for
//! standalone local development.
//!
//! # Environment
//!
//! - `BIND_ADDR`: the listen address (default `127.0.0.1:8788`), loopback
//!   only unless `SEAQUEL_ALLOW_NON_LOOPBACK=1`.
//! - `DATA_DIR`: the data root, shared with Node; users' files go under
//!   `DATA_DIR/users/`. Unset, it's the current directory, as in Node.
//!   `server.js` passes its environment and working directory through.
//!
//! At startup the soft open-files limit is raised towards the hard limit:
//! at the workspace LRU's cap the SQLite pools alone can hold about 6,000
//! descriptors.

use seaquel_server::startup::{self, ALLOW_NON_LOOPBACK_ENV, RECOMMENDED_NOFILE};
use seaquel_server::{build_router, AppState};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    startup::init_logging();

    // Loopback by default; see "Security: loopback only" above.
    let addr: SocketAddr = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8788".to_string())
        .parse()
        .expect("BIND_ADDR must be a valid socket address");
    let allow_non_loopback = startup::allow_non_loopback_from_env();
    if let Err(message) = startup::check_bind_addr(&addr, allow_non_loopback) {
        eprintln!("seaquel-server: {message}");
        std::process::exit(1);
    }
    if !addr.ip().is_loopback() {
        eprintln!(
            "seaquel-server: warning: binding to {addr}, which isn't loopback \
             ({ALLOW_NON_LOOPBACK_ENV}=1). Never do this in a deployment."
        );
    }

    match startup::raise_nofile_limit() {
        Some(lim) => {
            log::info!(
                "open-files limit: {} (was {}, hard {})",
                lim.after,
                lim.before,
                lim.hard
            );
            if lim.after < RECOMMENDED_NOFILE {
                log::warn!(
                    "open-files limit {} is below {RECOMMENDED_NOFILE}; with many web users \
                     the workspace pools can run out of file descriptors. Raise the hard \
                     limit (ulimit -Hn, or the container's nofile ulimit).",
                    lim.after
                );
            }
        }
        None => log::info!("open-files limit: not available on this platform"),
    }

    let state = AppState::default();
    log::info!(
        "seaquel-server data dir: {}",
        state.workspaces.root().display()
    );
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");

    log::info!("seaquel-server listening on {}", addr);
    axum::serve(listener, app).await.expect("server error");
}
