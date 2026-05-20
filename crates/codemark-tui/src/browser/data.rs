//! Database operations and background task handlers.

use crate::event::Event;
use codemark_core::config::Config;
use codemark_core::engine::bookmark::BookmarkHealth;
use codemark_core::engine::heal;
use codemark_core::storage::db::Database;

use super::types::HealTarget;

/// Perform heal operation in a background task.
pub async fn perform_heal(
    db_path: std::path::PathBuf,
    target: HealTarget,
    event_handler: crate::event::EventHandler,
) -> anyhow::Result<()> {
    let db = Database::open(&db_path)?;
    let Some(codemark_dir) = db_path.parent() else {
        let _ = event_handler
            .send(Event::HealComplete("Failed to determine codemark directory".to_string(), false));
        return Ok(());
    };

    let config = Config::load_layered(codemark_dir);
    let heal_options = heal::HealOptions {
        force: false,
        auto_archive: false,
        archive_after: config.health.auto_archive_days(),
        validate_only: false,
    };

    match target {
        HealTarget::Bookmark(bookmark_id) => {
            let bm_result = db.get_bookmark(&bookmark_id);
            if let Ok(Some(bm)) = bm_result {
                match heal::heal_bookmark(&db, &bm, &config, &heal_options).await {
                    Ok(result) => {
                        let status = match result.new_health {
                            BookmarkHealth::Active => "Active",
                            BookmarkHealth::Drifted => "Drifted",
                            BookmarkHealth::Stale => "Stale",
                            BookmarkHealth::Archived => "Archived",
                        };
                        let _ = event_handler.send(Event::HealComplete(
                            if result.previous_health != result.new_health {
                                format!("Healed: {} → {}", result.previous_health, status)
                            } else {
                                format!("Already {}", status)
                            },
                            true,
                        ));
                    }
                    Err(_) => {
                        let _ = event_handler
                            .send(Event::HealComplete("Heal failed".to_string(), false));
                    }
                }
            } else {
                let _ = event_handler
                    .send(Event::HealComplete("Bookmark not found".to_string(), false));
            }
        }
        HealTarget::Collection(collection_id) => {
            match heal::heal_collection(&db, &collection_id, &config, &heal_options).await {
                Ok(result) => {
                    let msg = if result.skipped > 0 {
                        format!("Healed {}, skipped {}", result.healed, result.skipped)
                    } else {
                        format!("Healed {}", result.healed)
                    };
                    let _ = event_handler.send(Event::HealComplete(msg, result.failed == 0));
                }
                Err(_) => {
                    let _ = event_handler.send(Event::HealComplete("Heal failed".to_string(), false));
                }
            }
        }
    }

    Ok(())
}
