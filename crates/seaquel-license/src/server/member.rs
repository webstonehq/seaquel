//! `member_license`: the local mirror of "this user holds this license in
//! this tenant", one row per Better Auth user, ported from
//! `member-license.ts`. The control plane's `tenant_members` is the
//! authoritative copy; the local row answers the gate without a network
//! round trip.

use std::fmt;

use sqlx::SqliteConnection;

use super::{LicenseServer, MemberState, Result};

/// One `member_license` row. `Debug` hides the key.
#[derive(Clone, PartialEq, Eq)]
pub struct MemberLicense {
    pub user_id: String,
    pub license_key: String,
    /// Unix seconds.
    pub bound_at: i64,
    pub control_member_id: String,
    pub is_owner: bool,
    /// Set by a bundle import's revocation walk (unix seconds).
    pub revoked_at: Option<i64>,
}

impl fmt::Debug for MemberLicense {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemberLicense")
            .field("user_id", &self.user_id)
            .field("license_key", &"<redacted>")
            .field("bound_at", &self.bound_at)
            .field("control_member_id", &self.control_member_id)
            .field("is_owner", &self.is_owner)
            .field("revoked_at", &self.revoked_at)
            .finish()
    }
}

impl MemberLicense {
    pub fn state(&self) -> MemberState {
        MemberState {
            is_owner: self.is_owner,
            revoked: self.revoked_at.is_some(),
        }
    }
}

type Row = (String, String, i64, String, i64, Option<i64>);

fn from_row(r: Row) -> MemberLicense {
    MemberLicense {
        user_id: r.0,
        license_key: r.1,
        bound_at: r.2,
        control_member_id: r.3,
        // `is_owner === 1`, as the TS read it.
        is_owner: r.4 == 1,
        revoked_at: r.5,
    }
}

const SELECT: &str =
    "SELECT user_id, license_key, bound_at, control_member_id, is_owner, revoked_at
                        FROM member_license";

impl LicenseServer {
    pub async fn find_member(&self, user_id: &str) -> Result<Option<MemberLicense>> {
        let row: Option<Row> = sqlx::query_as(&format!("{SELECT} WHERE user_id = ?"))
            .bind(user_id)
            .fetch_optional(self.pool().await?)
            .await?;
        Ok(row.map(from_row))
    }

    pub async fn find_member_by_license_key(&self, key: &str) -> Result<Option<MemberLicense>> {
        let row: Option<Row> = sqlx::query_as(&format!("{SELECT} WHERE license_key = ?"))
            .bind(key)
            .fetch_optional(self.pool().await?)
            .await?;
        Ok(row.map(from_row))
    }

    /// The owner's key, which authenticates admin calls upstream.
    pub(crate) async fn owner_license_key(&self) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT license_key FROM member_license WHERE is_owner = 1 LIMIT 1")
                .fetch_optional(self.pool().await?)
                .await?,
        )
    }

    /// A new row. `revoked_at` stays NULL until a bundle revokes the key.
    pub async fn insert_member(
        &self,
        user_id: &str,
        license_key: &str,
        bound_at: i64,
        control_member_id: &str,
        is_owner: bool,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO member_license
               (user_id, license_key, bound_at, control_member_id, is_owner)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(license_key)
        .bind(bound_at)
        .bind(control_member_id)
        .bind(is_owner as i64)
        .execute(self.pool().await?)
        .await?;
        Ok(())
    }

    pub async fn delete_member(&self, user_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM member_license WHERE user_id = ?")
            .bind(user_id)
            .execute(self.pool().await?)
            .await?;
        Ok(())
    }

    /// Every user whose row a bundle revoked, in id order.
    pub async fn revoked_user_ids(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT user_id FROM member_license WHERE revoked_at IS NOT NULL ORDER BY user_id",
        )
        .fetch_all(self.pool().await?)
        .await?)
    }

    /// Everyone bound and not revoked, joined to their email, oldest first.
    pub(crate) async fn active_members(&self) -> Result<Vec<(String, String, i64, i64, String)>> {
        Ok(sqlx::query_as(
            r#"SELECT ml.user_id, ml.license_key, ml.bound_at, ml.is_owner, u.email
                 FROM member_license ml
                 JOIN "user" u ON u.id = ml.user_id
                WHERE ml.revoked_at IS NULL
                ORDER BY ml.bound_at ASC"#,
        )
        .fetch_all(self.pool().await?)
        .await?)
    }
}

/// Stamp `revoked_at = now` on every unrevoked row whose key is in `keys`,
/// inside the caller's transaction. Returns the users whose rows changed.
pub(crate) async fn mark_revoked(
    conn: &mut SqliteConnection,
    keys: &[String],
    now: i64,
) -> Result<Vec<String>> {
    let mut users = Vec::new();
    // Chunked to stay under SQLite's bound-variable limit; one transaction
    // either way.
    for chunk in keys.chunks(500) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "UPDATE member_license SET revoked_at = ?
              WHERE license_key IN ({placeholders}) AND revoked_at IS NULL
              RETURNING user_id"
        );
        let mut q = sqlx::query_scalar::<_, String>(&sql).bind(now);
        for k in chunk {
            q = q.bind(k);
        }
        users.extend(q.fetch_all(&mut *conn).await?);
    }
    Ok(users)
}
