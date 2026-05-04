use anyhow::Result;
use codemark_core::storage::db::Database;
pub use codemark_core::storage::pack::inspector::{PackError, PackInfo, inspect, pre_inspect};
use rusqlite::Connection;

pub const CURRENT_SERVER_VERSION: i64 = 14;

pub fn migrate_pack_forward(conn: &mut Connection) -> Result<()> {
    Database::run_migrations_on(conn)?;
    Ok(())
}
