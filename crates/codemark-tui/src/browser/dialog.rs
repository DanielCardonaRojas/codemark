//! Modal dialog handling for the browser layout.
//!
//! Dialogs are modal overlays drawn on top of the layout. While one is active
//! it captures all key input so the user can only confirm or cancel the pending
//! action. They are currently used to confirm destructive operations such as
//! deleting bookmarks and collections.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::ui;

use super::{BrowserLayout, ConfirmDialog, DialogAction, DialogButton, HealNotification};

impl BrowserLayout {
    /// Whether a modal dialog is currently being displayed.
    ///
    /// While true, the dialog captures all key input and the rest of the
    /// layout (and global keybindings) must not act on events.
    pub fn has_active_dialog(&self) -> bool {
        self.dialog.is_some()
    }

    /// Show a confirmation dialog for the given action.
    pub(super) fn request_confirmation(&mut self, dialog: ConfirmDialog) {
        tracing::debug!(
            target: "codemark::ui",
            title = %dialog.title,
            action = ?dialog.action,
            "confirmation dialog opened"
        );
        self.dialog = Some(dialog);
    }

    /// Handle a key event while a dialog is active.
    ///
    /// ←/→/Tab move focus between the Cancel and Confirm buttons; Enter activates
    /// the focused button; Esc cancels outright. All other keys are swallowed so
    /// the dialog stays modal. Always returns `true`.
    pub(super) fn handle_dialog_key(&mut self, key: &ratatui::crossterm::event::KeyEvent) -> bool {
        use ratatui::crossterm::event::KeyCode;
        match key.code {
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Char('h')
            | KeyCode::Char('l') => {
                if let Some(dialog) = self.dialog.as_mut() {
                    dialog.selected = dialog.selected.toggle();
                }
            }
            KeyCode::Enter => {
                if let Some(dialog) = self.dialog.take() {
                    match dialog.selected {
                        DialogButton::Confirm => {
                            tracing::debug!(target: "codemark::ui", "confirmation dialog confirmed");
                            self.perform_dialog_action(dialog.action);
                        }
                        DialogButton::Cancel => {
                            tracing::debug!(target: "codemark::ui", "confirmation dialog cancelled");
                        }
                    }
                }
            }
            KeyCode::Esc => {
                tracing::debug!(target: "codemark::ui", "confirmation dialog cancelled");
                self.dialog = None;
            }
            _ => {}
        }
        true
    }

    /// Execute a confirmed dialog action, refreshing panels on success.
    ///
    /// A failed deletion is surfaced as an error notification rather than
    /// silently swallowed, so the UI never closes the modal as if a destructive
    /// action succeeded when it did not.
    fn perform_dialog_action(&mut self, action: DialogAction) {
        let result = match &action {
            DialogAction::DeleteBookmark(id) => {
                tracing::debug!(target: "codemark::ui", %id, "deleting bookmark");
                self.db.delete_bookmark(id).map(|_| ())
            }
            DialogAction::DeleteCollection(id) => {
                tracing::debug!(target: "codemark::ui", %id, "deleting collection and its bookmarks");
                self.db.delete_collection_recursive(id).map(|_| ())
            }
        };

        match result {
            Ok(()) => self.refresh_all_panels(),
            Err(err) => {
                tracing::warn!(
                    target: "codemark::ui",
                    error = %err,
                    "confirmed delete action failed"
                );
                self.pending_notification = Some(HealNotification {
                    message: format!("Delete failed: {}", err),
                    success: false,
                });
            }
        }
    }

    /// Render the active dialog (if any) centered over the given area.
    pub(super) fn render_dialog(&self, area: Rect, buf: &mut Buffer) {
        if let Some(dialog) = &self.dialog {
            let confirm_selected = dialog.selected == DialogButton::Confirm;
            ui::render_confirmation(area, buf, &dialog.title, &dialog.message, confirm_selected);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::browser::{BrowserLayout, ConfirmDialog, DialogAction, DialogButton};
    use crate::event::{EventHandler, EventHandlerConfig};
    use codemark_core::storage::db::Database;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};

    /// Build a layout backed by a throwaway database. The returned `TempDir`
    /// must be kept alive for the duration of the test; it is cleaned up when
    /// it goes out of scope at the end of the test.
    fn test_layout() -> (BrowserLayout, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open db");
        let handler = EventHandler::new(EventHandlerConfig::default()).expect("event handler");
        let layout = BrowserLayout::new(db, handler);
        (layout, dir)
    }

    fn sample_dialog() -> ConfirmDialog {
        ConfirmDialog::new(
            "Delete Bookmark",
            "Delete bookmark \"foo\"?",
            DialogAction::DeleteBookmark("missing-id".to_string()),
        )
    }

    #[test]
    fn no_dialog_by_default() {
        let (layout, _dir) = test_layout();
        assert!(!layout.has_active_dialog());
    }

    #[test]
    fn requesting_confirmation_activates_dialog_focused_on_cancel() {
        let (mut layout, _dir) = test_layout();
        layout.request_confirmation(sample_dialog());
        assert!(layout.has_active_dialog());
        // Destructive dialogs start on Cancel for safety.
        assert_eq!(layout.dialog.as_ref().unwrap().selected, DialogButton::Cancel);
    }

    #[test]
    fn arrow_keys_toggle_button_focus() {
        let (mut layout, _dir) = test_layout();
        layout.request_confirmation(sample_dialog());

        layout.handle_dialog_key(&KeyEvent::from(KeyCode::Right));
        assert_eq!(layout.dialog.as_ref().unwrap().selected, DialogButton::Confirm);

        layout.handle_dialog_key(&KeyEvent::from(KeyCode::Left));
        assert_eq!(layout.dialog.as_ref().unwrap().selected, DialogButton::Cancel);
    }

    #[test]
    fn enter_on_cancel_dismisses_without_action() {
        let (mut layout, _dir) = test_layout();
        layout.request_confirmation(sample_dialog());

        // Default focus is Cancel, so Enter just closes the dialog.
        let handled = layout.handle_dialog_key(&KeyEvent::from(KeyCode::Enter));
        assert!(handled);
        assert!(!layout.has_active_dialog());
    }

    #[test]
    fn esc_key_dismisses_dialog() {
        let (mut layout, _dir) = test_layout();
        layout.request_confirmation(sample_dialog());

        layout.handle_dialog_key(&KeyEvent::from(KeyCode::Esc));
        assert!(!layout.has_active_dialog());
    }

    #[test]
    fn enter_on_confirm_runs_action_and_closes_dialog() {
        let (mut layout, _dir) = test_layout();
        layout.request_confirmation(sample_dialog());

        // Move focus to Confirm, then activate it. Deleting a missing id is a
        // no-op on the DB but must still close the dialog without panicking.
        layout.handle_dialog_key(&KeyEvent::from(KeyCode::Tab));
        let handled = layout.handle_dialog_key(&KeyEvent::from(KeyCode::Enter));
        assert!(handled);
        assert!(!layout.has_active_dialog());
    }

    #[test]
    fn unrelated_keys_keep_dialog_open() {
        let (mut layout, _dir) = test_layout();
        layout.request_confirmation(sample_dialog());

        let handled = layout.handle_dialog_key(&KeyEvent::from(KeyCode::Char('x')));
        // Modal: the key is swallowed but the dialog stays open.
        assert!(handled);
        assert!(layout.has_active_dialog());
    }
}
