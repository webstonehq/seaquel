use sqlx::{MySql, Pool};

use seaquel_engine::{ConnectConfig, DbError};

pub struct MysqlDriver {
    pool: Pool<MySql>,
}

impl MysqlDriver {
    pub async fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        let conn_str = config
            .connection_string
            .as_deref()
            .ok_or_else(|| DbError::connection_error("connection_string is required for MySQL"))?;

        let pool = Pool::<MySql>::connect(conn_str)
            .await
            .map_err(DbError::connection_error)?;

        Ok(Self { pool })
    }
}

seaquel_engine::impl_sqlx_driver!(
    MysqlDriver,
    MySql,
    sqlx::mysql::MySqlArguments,
    decode_fn = crate::decode::to_json,
    last_insert_id = |r: &sqlx::mysql::MySqlQueryResult| Some(r.last_insert_id() as i64)
);
