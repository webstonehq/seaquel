use sqlx::{Pool, Postgres};

use super::{ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};

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
            .map_err(|e| DbError::connection_error(e))?;

        Ok(Self { pool })
    }
}

super::impl_sqlx_driver!(
    PostgresDriver,
    Postgres,
    sqlx::postgres::PgArguments,
    decode_fn = super::decode::postgres::to_json,
    last_insert_id = |_: &_| None
);
