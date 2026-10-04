//! Test support: model fixtures, scripted keys, the snapshot helper (spike
//! S3), a seeded Core in a temp data dir and a harness that drives the TUI
//! over it. Compiled for tests only.

pub mod core;
pub mod fixtures;
pub mod harness;
pub mod keys;
pub mod live;
pub mod snapshot;
