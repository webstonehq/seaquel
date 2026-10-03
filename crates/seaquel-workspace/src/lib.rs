//! The workspace domain: logic over one user's saved data that every
//! interface shares.
//!
//! Its first module (phase 4) turns a stored connection plus its keychain
//! secrets into a `ConnectConfig`, a port of the TypeScript in
//! `connection-manager.svelte.ts`, `connection-string.ts` and `wire.ts`.
//! Phase 5 moves the GUI's services here: [`run`] plans the editor's runs
//! (phase 5b), and [`edits`] the grid's edits and the data tab's query
//! (phase 5c), and [`library`] the saved connections, projects, labels and
//! saved queries Core writes (phase 5d), and [`shared`] the `.seaquel`
//! files of shared projects and their sync (phase 5e), and [`ai`] the
//! assistant's wire types (phase 6). Interfaces reach it only through
//! Core's `workspace` feature (`seaquel_core::domain`), and it may not name
//! an engine crate (`scripts/check-crate-deps.mjs`).

pub mod ai;
pub mod connection_string;
pub mod connections;
pub mod edits;
pub mod imports;
pub mod library;
pub mod run;
pub mod shared;
pub mod shared_api;
pub mod state;
