//! Air-gapped installs: signed bundles from the control plane stand in for
//! it. `canonical` and `verify` are pure; `bundle_store` keeps the one
//! active bundle in `auth.db`; `local_control` answers the control-plane
//! calls from it.

use std::collections::HashMap;

pub mod bundle_store;
pub mod canonical;
pub mod local_control;
pub mod verify;

/// Trusted Ed25519 public keys by fingerprint (32 lowercase hex characters
/// of `sha256(pubkey)`).
pub type TrustSet = HashMap<String, Vec<u8>>;
