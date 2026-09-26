//! The active air-gap bundle (`airgap_bundle`, one row) and the trusted
//! keys, ported from `bundle-store.ts`.
//!
//! The row keeps the raw signed envelope, which is re-verified on every
//! read that misses the in-process cache: a row edited without the signing
//! key, or signed by a key no longer trusted, reads as no bundle. The cache
//! holds one parsed bundle, keyed by the row's `payload_sha256`.

use std::sync::{Arc, PoisonError};

use sqlx::SqliteConnection;

use super::verify::{verify_bundle, BundlePayload, VerifiedBundle};
use super::TrustSet;
use crate::server::js::{js_trim, parse_int, to_i64};
use crate::server::{member, LicenseServer, Result};

/// The bundle in use.
#[derive(Debug, Clone)]
pub struct ActiveBundle {
    pub payload: BundlePayload,
    pub raw_envelope: Vec<u8>,
    pub pubkey_fingerprint: String,
    /// Unix seconds.
    pub imported_at: i64,
    pub payload_sha256: String,
}

/// The production trust set: fingerprint and hex public key of the
/// seaquel.app bundle-signing key(s).
pub const PROD_TRUSTED_PUBKEYS: &[(&str, &str)] = &[(
    "937769290e77859410e977c3f762aa0c",
    "dab5fab2c0c17da9a5c0f3ce2a710a496ff3db92578e09b2d786f44e99411a5e",
)];

/// `hexToBytes`: an optional `0x`, an even length, then each pair read by
/// `parseInt(pair, 16)` and stored as a `Uint8Array` would (mod 256).
fn hex_to_bytes(hex: &str) -> std::result::Result<Vec<u8>, &'static str> {
    let clean = hex.strip_prefix("0x").unwrap_or(hex);
    let units: Vec<u16> = clean.encode_utf16().collect();
    if !units.len().is_multiple_of(2) {
        return Err("invalid trusted pubkey hex (odd length)");
    }
    units
        .chunks(2)
        .map(|pair| {
            let pair = String::from_utf16_lossy(pair);
            parse_int(&pair, 16)
                .map(|n| n.rem_euclid(256) as u8)
                .ok_or("invalid trusted pubkey hex (non-hex char)")
        })
        .collect()
}

/// The keys bundles may be signed with: the production set, then each
/// `<fingerprint>:<hex>` entry of `spec` (`SEAQUEL_BUNDLE_TRUSTED_PUBKEY`,
/// comma-separated) on top. Malformed entries are skipped with a warning.
pub fn load_trusted_pubkeys(spec: Option<&str>) -> TrustSet {
    let mut out = TrustSet::new();
    for (fingerprint, hex) in PROD_TRUSTED_PUBKEYS {
        if let Ok(key) = hex_to_bytes(hex) {
            out.insert((*fingerprint).to_string(), key);
        }
    }
    let Some(spec) = spec.filter(|s| !s.is_empty()) else {
        return out;
    };
    for raw in spec.split(',') {
        let trimmed = js_trim(raw);
        if trimmed.is_empty() {
            continue;
        }
        let colon = trimmed.find(':');
        let Some(colon) = colon.filter(|&c| c > 0 && c != trimmed.len() - 1) else {
            log::warn!(activity = "license.trust"; "SEAQUEL_BUNDLE_TRUSTED_PUBKEY entry ignored (expected <fingerprint>:<hex>): {trimmed}");
            continue;
        };
        let fingerprint = js_trim(&trimmed[..colon]);
        let hex = js_trim(&trimmed[colon + 1..]);
        match hex_to_bytes(hex) {
            Ok(key) => {
                out.insert(fingerprint.to_string(), key);
            }
            Err(why) => {
                log::warn!(activity = "license.trust"; "SEAQUEL_BUNDLE_TRUSTED_PUBKEY entry ignored ({why}): {trimmed}");
            }
        }
    }
    out
}

type BundleRow = (Vec<u8>, String, i64, String);

impl LicenseServer {
    /// The stored bundle, re-verified against this server's trust set, or
    /// `None` when there is none or it no longer verifies.
    pub async fn read_active_bundle(&self) -> Result<Option<Arc<ActiveBundle>>> {
        let row: Option<BundleRow> = sqlx::query_as(
            "SELECT raw_envelope, pubkey_fingerprint, imported_at, payload_sha256
               FROM airgap_bundle WHERE id = 1",
        )
        .fetch_optional(self.pool().await?)
        .await?;
        let Some((raw_envelope, _, imported_at, payload_sha256)) = row else {
            return Ok(None);
        };

        let cached = self
            .bundle_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(b) = cached.filter(|b| b.payload_sha256 == payload_sha256) {
            return Ok(Some(b));
        }

        match verify_bundle(&raw_envelope, &self.config.trusted, self.now()) {
            // The hash column must match the payload that verified: a row
            // edited to point at another hash reads as no bundle.
            Ok(verified) if verified.payload.sha256_hex() != payload_sha256 => {
                log::warn!(activity = "license.airgap"; "airgap_bundle payload_sha256 doesn't match its verified payload; treating as no bundle");
                self.drop_bundle_cache();
                Ok(None)
            }
            Ok(verified) => {
                let active = Arc::new(ActiveBundle {
                    payload: verified.payload,
                    raw_envelope: verified.raw_envelope,
                    pubkey_fingerprint: verified.pubkey_fingerprint,
                    imported_at,
                    payload_sha256,
                });
                *self
                    .bundle_cache
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(active.clone());
                Ok(Some(active))
            }
            Err(e) => {
                log::warn!(activity = "license.airgap", code = e.as_str(); "airgap_bundle re-verification failed: {e}; treating as no bundle");
                self.drop_bundle_cache();
                Ok(None)
            }
        }
    }

    /// Store a verified bundle (replacing any), `imported_at` now.
    pub async fn write_bundle(
        &self,
        verified: &VerifiedBundle,
        payload_sha256: &str,
    ) -> Result<()> {
        let mut conn = self.pool().await?.acquire().await?;
        write_bundle_row(&mut conn, verified, payload_sha256, self.now()).await?;
        self.drop_bundle_cache();
        Ok(())
    }

    /// Store a bundle and apply its revocations in one transaction. Returns
    /// the users whose rows were revoked.
    pub(crate) async fn import_bundle(
        &self,
        verified: &VerifiedBundle,
        payload_sha256: &str,
    ) -> Result<Vec<String>> {
        let now = self.now();
        let mut tx = self.pool().await?.begin().await?;
        // The write comes first, so the transaction takes the write lock
        // (waiting out Node's) before it reads anything.
        write_bundle_row(&mut tx, verified, payload_sha256, now).await?;
        let users = member::mark_revoked(&mut tx, &verified.payload.revoked_keys, now).await?;
        tx.commit().await?;
        self.drop_bundle_cache();
        Ok(users)
    }

    pub async fn clear_bundle(&self) -> Result<()> {
        sqlx::query("DELETE FROM airgap_bundle WHERE id = 1")
            .execute(self.pool().await?)
            .await?;
        self.drop_bundle_cache();
        Ok(())
    }

    /// Whether a bundle row exists, without re-verifying it: the switch
    /// between the control plane and the local answers.
    pub async fn is_bundle_driven(&self) -> Result<bool> {
        let row: Option<i64> = sqlx::query_scalar("SELECT 1 FROM airgap_bundle WHERE id = 1")
            .fetch_optional(self.pool().await?)
            .await?;
        Ok(row.is_some())
    }

    fn drop_bundle_cache(&self) {
        *self
            .bundle_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }
}

async fn write_bundle_row(
    conn: &mut SqliteConnection,
    verified: &VerifiedBundle,
    payload_sha256: &str,
    imported_at: i64,
) -> Result<()> {
    // `JSON.stringify(payload)`: a projection for people reading the DB;
    // the raw envelope is what counts.
    let verified_payload = serde_json::to_string(&verified.payload).unwrap_or_default();
    sqlx::query(
        "INSERT OR REPLACE INTO airgap_bundle
           (id, raw_envelope, verified_payload, pubkey_fingerprint,
            imported_at, not_after, payload_sha256, issued_at)
         VALUES (1, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&verified.raw_envelope)
    .bind(verified_payload)
    .bind(&verified.pubkey_fingerprint)
    .bind(imported_at)
    .bind(to_i64(verified.payload.not_after))
    .bind(payload_sha256)
    .bind(to_i64(verified.payload.issued_at))
    .execute(conn)
    .await?;
    Ok(())
}
