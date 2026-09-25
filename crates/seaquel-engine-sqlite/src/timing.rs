//! Wall-clock timing for SQLite's EXPLAIN ANALYZE, which runs the statement
//! itself.
//!
//! `crates/clippy.toml` bans `std::time::Instant` because Core builds for
//! wasm32, where it panics. Engine crates are native only (they link sqlx),
//! so the monotonic clock is fine here.

use std::future::Future;

/// `f`'s output and how long it took, in milliseconds.
#[allow(clippy::disallowed_types)]
pub async fn timed<T>(f: impl Future<Output = T>) -> (T, f64) {
    let start = std::time::Instant::now();
    let out = f.await;
    (out, start.elapsed().as_secs_f64() * 1000.0)
}
