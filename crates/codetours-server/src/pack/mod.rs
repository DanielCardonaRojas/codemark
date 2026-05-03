pub mod inspector;

pub use inspector::{inspect, pre_inspect, PackError, PackInfo};
use rusqlite::Connection;
use codemark_core::storage::db::Database;
use anyhow::Result;

pub const CURRENT_SERVER_VERSION: i64 = 12;

pub fn migrate_pack_forward(conn: &mut Connection) -> Result<()> {
    Database::run_migrations_on(conn)?;
    Ok(())
}
