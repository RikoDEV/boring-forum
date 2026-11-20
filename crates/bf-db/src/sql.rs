//! Small shared query shapes. SQL is assembled with `concat!` from literals, so it is a
//! `&'static str` at compile time and never goes through `AssertSqlSafe`.

/// `pub async fn name(pool, id, value) -> Result<(), Error>` running `UPDATE table SET col = $2
/// WHERE id = $1`.
macro_rules! set_col {
    ($(#[$m:meta])* $name:ident, $table:literal, $col:literal, $id:ty, $val:ty) => {
        $(#[$m])*
        pub async fn $name(pool: &sqlx::PgPool, id: $id, value: $val) -> Result<(), sqlx::Error> {
            sqlx::query(concat!("UPDATE ", $table, " SET ", $col, " = $2 WHERE id = $1"))
                .bind(id)
                .bind(value)
                .execute(pool)
                .await
                .map(|_| ())
        }
    };
}

/// `pub async fn name(pool, id) -> Result<bool, Error>` running `DELETE FROM table WHERE id = $1`;
/// true when a row went.
macro_rules! delete_by_id {
    ($(#[$m:meta])* $name:ident, $table:literal) => {
        $(#[$m])*
        pub async fn $name(pool: &sqlx::PgPool, id: i64) -> Result<bool, sqlx::Error> {
            Ok(sqlx::query(concat!("DELETE FROM ", $table, " WHERE id = $1"))
                .bind(id)
                .execute(pool)
                .await?
                .rows_affected()
                > 0)
        }
    };
}

pub(crate) use {delete_by_id, set_col};
