//! `onboardingRepo`: `onboarding_state`, one row holding JSON.

use serde_json::value::RawValue;

use super::codec::{load_singleton_json, save_singleton_json, Result};
use crate::Storage;

const TABLE: &str = "onboarding_state";

/// The onboarding state as its stored JSON. `None` (JSON `null`) when there's no
/// row or the stored text doesn't parse; stored `null` loads as `null` too.
pub async fn load(st: &Storage) -> Result<Option<Box<RawValue>>> {
    load_singleton_json(st, TABLE).await
}

/// Stores `data` as the JSON given (`null` is stored as `'null'`).
pub async fn save(st: &Storage, data: &RawValue) -> Result<()> {
    save_singleton_json(st, TABLE, data).await
}
