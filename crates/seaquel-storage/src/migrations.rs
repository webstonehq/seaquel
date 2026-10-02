//! The numbered migrations in `migrations/`, embedded by `build.rs` for
//! the in-memory executor's migrator (phase 8 Decision 4), which records
//! each one in `_sqlx_migrations` exactly as sqlx's does: the version and
//! description sqlx reads from the file name, and the SHA-384 of the
//! file's text as its checksum. So a file made in the browser and one made
//! on desktop are interchangeable. Natively `sqlx::migrate!` reads the same
//! files; a test checks the two lists agree.

/// One migration file.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Migration {
    pub version: i64,
    /// sqlx's description: the name after the version, `_` as spaces.
    pub description: &'static str,
    pub sql: &'static str,
}

impl Migration {
    /// sqlx's checksum: SHA-384 of the file's text.
    pub(crate) fn checksum(&self) -> Vec<u8> {
        use sha2::{Digest, Sha384};
        Sha384::digest(self.sql.as_bytes()).to_vec()
    }
}

include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    /// The embedded list is sqlx's own: same versions, descriptions and
    /// checksums, in the same order.
    #[test]
    fn migrations_match_sqlx() {
        let sqlx = crate::db::embedded_migrator();
        let theirs: Vec<(i64, String, Vec<u8>)> = sqlx
            .iter()
            .filter(|m| !m.migration_type.is_down_migration())
            .map(|m| (m.version, m.description.to_string(), m.checksum.to_vec()))
            .collect();
        let ours: Vec<(i64, String, Vec<u8>)> = MIGRATIONS
            .iter()
            .map(|m| (m.version, m.description.to_string(), m.checksum()))
            .collect();
        assert_eq!(ours, theirs);
        assert_eq!(ours.len(), 6);
        assert_eq!(ours[5].1, "history params");
        assert_eq!(ours[0].1, "name keys");
    }
}
