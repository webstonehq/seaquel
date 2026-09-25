use sqlx::{Pool, Postgres};

use seaquel_engine::{ConnectConfig, DbError};

pub struct PostgresDriver {
    pool: Pool<Postgres>,
}

impl PostgresDriver {
    pub async fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        let conn_str = config
            .connection_string
            .as_deref()
            .ok_or_else(|| DbError::connection_error("connection_string is required for PostgreSQL"))?;

        let pool = Pool::<Postgres>::connect(conn_str)
            .await
            .map_err(DbError::connection_error)?;

        Ok(Self { pool })
    }
}

seaquel_engine::impl_sqlx_driver!(
    PostgresDriver,
    Postgres,
    sqlx::postgres::PgArguments,
    decode_fn = crate::decode::to_json,
    last_insert_id = |_: &_| None
);
