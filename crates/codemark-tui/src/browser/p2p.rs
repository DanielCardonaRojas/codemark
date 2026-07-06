//! Peer-to-peer tour sharing for the browser layout (feature `p2p`).
//!
//! All p2p UI lives here: a push-method chooser, a paste-ticket modal, and a
//! serving menu, plus the background tasks that serve/pull tours over the
//! `codemark-p2p` transport. Everything is gated behind the `p2p` feature, so a
//! default TUI build contains none of it.
//!
//! The transport is tour-agnostic; this module bridges it to tours via the
//! `codemark-core` pack helpers (`build_pack_bytes` / `import_pack_bytes`),
//! reopening the (non-`Send`) `Database` by path inside background tasks exactly
//! like the existing server push/pull.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget, Wrap};

use codemark_core::config::Config;
use codemark_core::storage::db::Database;

use super::tabs::ContentTab;
use super::{BrowserLayout, FocusArea, HealNotification};
use crate::event::Event;

/// State for an active p2p push: the tour is being served over iroh until the
/// user stops it or the app quits.
pub struct P2pServing {
    /// Collection name being served (for the indicator and menu).
    pub name: String,
    /// The shareable ticket, once the serving task has minted it.
    pub ticket: Option<String>,
    /// Dropping this sender signals the serving task to shut the iroh node down.
    _stop: tokio::sync::oneshot::Sender<()>,
}

/// The active p2p modal overlay.
pub enum P2pModal {
    /// Choose how to push a collection: Codetours server or peer-to-peer.
    PushMethod { collection_id: String, name: String, peer_selected: bool },
    /// Paste a ticket to pull a tour.
    TicketInput { value: String },
    /// Menu shown while serving: re-copy ticket / stop / close.
    ServingMenu { index: usize },
}

/// Heuristic: does this string look like an iroh blob ticket? Used to decide
/// whether to pre-fill the paste modal from the clipboard.
fn looks_like_ticket(s: &str) -> bool {
    let s = s.trim();
    s.starts_with("blob") && s.len() > 8 && s.chars().all(|c| c.is_ascii_alphanumeric())
}

impl BrowserLayout {
    // ── Entry points (called from the key dispatcher) ────────────────────────

    /// Handle `P` on the Collections tab: offer a method choice if a Codetours
    /// server is configured, otherwise go straight to peer-to-peer serving.
    pub(super) fn handle_collection_push_key(&mut self) -> bool {
        let Some((collection_id, name)) = self.selected_collection() else {
            self.notify("No collection selected", false);
            return true;
        };
        if self.p2p_server_configured() {
            self.p2p_modal =
                Some(P2pModal::PushMethod { collection_id, name, peer_selected: true });
        } else {
            self.start_p2p_serving(collection_id, name);
        }
        true
    }

    /// Handle `p` on the Collections tab: open the paste-ticket modal,
    /// pre-filled from the clipboard when it holds something ticket-shaped.
    pub(super) fn handle_collection_pull_key(&mut self) -> bool {
        let prefill = self.read_clipboard().filter(|s| looks_like_ticket(s)).unwrap_or_default();
        self.p2p_modal = Some(P2pModal::TicketInput { value: prefill });
        true
    }

    /// Handle `Ctrl+E`: open the serving menu if a tour is being served.
    pub(super) fn open_serving_menu(&mut self) -> bool {
        if self.p2p_serving.is_some() {
            self.p2p_modal = Some(P2pModal::ServingMenu { index: 1 });
            true
        } else {
            false
        }
    }

    /// Whether a tour is currently being served (drives the bottom-bar hint).
    pub(super) fn is_p2p_serving(&self) -> bool {
        self.p2p_serving.is_some()
    }

    // ── Modal input ──────────────────────────────────────────────────────────

    /// Whether a p2p modal is capturing input.
    pub(super) fn p2p_modal_active(&self) -> bool {
        self.p2p_modal.is_some()
    }

    /// Route a key to the active p2p modal. Returns true (input consumed).
    pub(super) fn handle_p2p_modal_key(&mut self, key: &KeyEvent) -> bool {
        if key.code == KeyCode::Esc {
            self.p2p_modal = None;
            return true;
        }

        // Navigation / text editing mutate the modal in place.
        match self.p2p_modal.as_mut() {
            Some(P2pModal::PushMethod { peer_selected, .. }) => match key.code {
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Tab
                | KeyCode::BackTab
                | KeyCode::Char('h')
                | KeyCode::Char('l') => {
                    *peer_selected = !*peer_selected;
                    return true;
                }
                _ => {}
            },
            Some(P2pModal::TicketInput { value }) => match key.code {
                KeyCode::Char(c) => {
                    value.push(c);
                    return true;
                }
                KeyCode::Backspace => {
                    value.pop();
                    return true;
                }
                _ => {}
            },
            Some(P2pModal::ServingMenu { index }) => match key.code {
                KeyCode::Left | KeyCode::Up | KeyCode::Char('h') | KeyCode::Char('k') => {
                    *index = index.saturating_sub(1);
                    return true;
                }
                KeyCode::Right
                | KeyCode::Down
                | KeyCode::Char('l')
                | KeyCode::Char('j')
                | KeyCode::Tab => {
                    *index = (*index + 1).min(2);
                    return true;
                }
                _ => {}
            },
            None => return false,
        }

        // Enter commits the modal. Take it first so the borrow is released
        // before dispatching to methods that need `&mut self`.
        if key.code == KeyCode::Enter {
            match self.p2p_modal.take() {
                Some(P2pModal::PushMethod { collection_id, name, peer_selected }) => {
                    if peer_selected {
                        self.start_p2p_serving(collection_id, name);
                    } else {
                        self.start_push_collection();
                    }
                }
                Some(P2pModal::TicketInput { value }) => {
                    let ticket = value.trim().to_string();
                    if ticket.is_empty() {
                        self.notify("No ticket entered", false);
                    } else {
                        self.start_p2p_pull(ticket);
                    }
                }
                Some(P2pModal::ServingMenu { index }) => match index {
                    0 => self.recopy_ticket(),
                    1 => self.stop_serving(),
                    _ => {}
                },
                None => {}
            }
        }
        true
    }

    /// Route a bracketed-paste into the ticket modal (reliable for long tickets).
    pub(super) fn handle_p2p_paste(&mut self, text: &str) -> bool {
        if let Some(P2pModal::TicketInput { value }) = self.p2p_modal.as_mut() {
            value.push_str(text.trim());
            true
        } else {
            false
        }
    }

    // ── Background push / pull ────────────────────────────────────────────────

    /// Build the tour pack and start serving it over iroh in the background.
    fn start_p2p_serving(&mut self, collection_id: String, name: String) {
        if self.p2p_serving.is_some() {
            self.notify("Already serving a tour — stop it first (Ctrl+E)", false);
            return;
        }
        let Some(dir) = self.db.path().parent().map(|d| d.to_path_buf()) else {
            self.notify("Failed to locate the codemark directory", false);
            return;
        };
        let db_path = self.db.path().to_path_buf();
        let config = Config::load_layered(&dir);
        let project_root = dir.parent().unwrap_or(&dir).to_path_buf();
        let eh = self.event_handler.clone();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let name_for_task = name.clone();

        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();
            // Build the pack bytes with the (non-Send) DB, then drop it.
            let built = handle.block_on(async {
                let db = Database::open(&db_path)?;
                codemark_core::sync::build_pack_bytes(
                    &db,
                    &collection_id,
                    &project_root,
                    &config,
                    None,
                    None,
                )
                .await
            });

            match built {
                Ok(bytes) => {
                    // Serve on the async runtime (not this blocking thread) so we
                    // don't pin a blocking worker for the whole serving duration.
                    handle.spawn(async move {
                        match codemark_p2p::push_bytes(bytes).await {
                            Ok((ticket, mut provider)) => {
                                let _ = eh.send(Event::P2pServing { name: name_for_task, ticket });
                                // Stop when the user asks, or automatically once a
                                // peer has downloaded the tour.
                                tokio::select! {
                                    _ = stop_rx => {}
                                    Some(()) = provider.recv_delivery() => {
                                        let _ = eh.send(Event::P2pDelivered);
                                    }
                                }
                                let _ = provider.shutdown().await;
                                let _ = eh.send(Event::P2pServingStopped);
                            }
                            Err(e) => {
                                let _ = eh.send(Event::SyncComplete(
                                    format!("p2p push failed: {e:#}"),
                                    false,
                                ));
                                let _ = eh.send(Event::P2pServingStopped);
                            }
                        }
                    });
                }
                Err(e) => {
                    let _ =
                        eh.send(Event::SyncComplete(format!("Failed to prepare tour: {e}"), false));
                    let _ = eh.send(Event::P2pServingStopped);
                }
            }
        });

        self.p2p_serving = Some(P2pServing { name, ticket: None, _stop: stop_tx });
        self.notify("Preparing tour for peer-to-peer sharing…", true);
    }

    /// Pull and import a tour from a ticket in the background.
    fn start_p2p_pull(&mut self, ticket: String) {
        let db_path = self.db.path().to_path_buf();
        let eh = self.event_handler.clone();

        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();
            let result: Result<codemark_core::sync::ImportedTour, String> =
                handle.block_on(async {
                    let bytes =
                        codemark_p2p::pull_bytes(&ticket).await.map_err(|e| format!("{e:#}"))?;
                    let db = Database::open(&db_path).map_err(|e| e.to_string())?;
                    codemark_core::sync::import_pack_bytes(&db, bytes, None, "p2p")
                        .await
                        .map_err(|e| e.to_string())
                });

            let (message, success) = match result {
                Ok(t) => (
                    format!(
                        "Imported '{}' ({} bookmark{})",
                        t.name,
                        t.bookmark_count,
                        if t.bookmark_count == 1 { "" } else { "s" }
                    ),
                    true,
                ),
                Err(e) => (format!("p2p pull failed: {e}"), false),
            };
            let _ = eh.send(Event::P2pPullComplete { message, success });
        });

        self.notify("Pulling tour over peer-to-peer…", true);
    }

    /// Re-copy the current ticket to the clipboard from the serving menu.
    fn recopy_ticket(&mut self) {
        let ticket = self.p2p_serving.as_ref().and_then(|s| s.ticket.clone());
        match ticket {
            Some(t) => match self.copy_to_clipboard(&t) {
                Ok(()) => self.notify("Ticket copied to clipboard", true),
                Err(e) => self.notify(format!("Failed to copy: {e}"), false),
            },
            None => self.notify("Ticket not ready yet", false),
        }
    }

    /// Stop the active serving task (drops the provider, shutting the node down).
    fn stop_serving(&mut self) {
        if let Some(serving) = self.p2p_serving.take() {
            self.notify(format!("Stopped serving '{}'", serving.name), true);
        }
    }

    // ── Event handlers (called from handle_app_event) ─────────────────────────

    /// A serving task minted its ticket: record it, copy it, and confirm.
    pub(super) fn on_p2p_serving(&mut self, name: &str, ticket: &str) {
        if let Some(serving) = self.p2p_serving.as_mut() {
            serving.name = name.to_string();
            serving.ticket = Some(ticket.to_string());
        }
        match self.copy_to_clipboard(ticket) {
            Ok(()) => self.notify(format!("Ticket copied — serving '{name}'"), true),
            Err(_) => {
                self.notify(format!("Serving '{name}' — press Ctrl+E to copy the ticket"), true)
            }
        }
    }

    /// A peer downloaded the served tour. Confirm it; the serving task then
    /// auto-stops and clears the indicator via `on_p2p_serving_stopped`.
    pub(super) fn on_p2p_delivered(&mut self) {
        let name = self.p2p_serving.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        self.notify(format!("Downloaded by peer — '{name}' delivered"), true);
    }

    /// The serving task ended; clear state without a toast (the delivery or a
    /// failure path has already surfaced its own message).
    pub(super) fn on_p2p_serving_stopped(&mut self) {
        self.p2p_serving = None;
    }

    /// A p2p pull finished: notify and, on success, refresh the Collections tab.
    pub(super) fn on_p2p_pull_complete(&mut self, message: &str, success: bool) {
        self.notify(message.to_string(), success);
        if success {
            self.refresh_all_panels();
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn selected_collection(&self) -> Option<(String, String)> {
        if self.focus != FocusArea::ContentPanel {
            return None;
        }
        if !matches!(
            ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index()),
            Some(ContentTab::Collections)
        ) {
            return None;
        }
        let panel = self.left_pane.content_panel.active_panel()?;
        let selected = panel.selected()?;
        let name = selected.text().to_string();
        let id = selected
            .user_data
            .clone()
            .or_else(|| self.db.get_collection_by_name(&name).ok().flatten().map(|c| c.id))?;
        Some((id, name))
    }

    fn p2p_server_configured(&self) -> bool {
        let Some(dir) = self.db.path().parent() else {
            return false;
        };
        let config = Config::load_layered(dir);
        codemark_core::sync::resolve_server_and_token(&config).is_ok()
    }

    fn read_clipboard(&mut self) -> Option<String> {
        use copypasta::ClipboardProvider;
        if self.clipboard.is_none() {
            self.clipboard = copypasta::ClipboardContext::new().ok();
        }
        self.clipboard.as_mut()?.get_contents().ok()
    }

    fn notify(&mut self, message: impl Into<String>, success: bool) {
        self.pending_notification = Some(HealNotification { message: message.into(), success });
    }

    // ── Rendering ─────────────────────────────────────────────────────────────

    /// Draw the active p2p modal, if any (called after the confirm dialog).
    pub(super) fn render_p2p_modal(&self, area: Rect, buf: &mut Buffer) {
        let Some(modal) = &self.p2p_modal else {
            return;
        };
        match modal {
            P2pModal::PushMethod { name, peer_selected, .. } => {
                let body = vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        format!("Share '{name}' via:"),
                        Style::default().bold(),
                    )),
                    Line::from(""),
                    Line::from(vec![
                        button("  Codetours server  ", !*peer_selected),
                        Span::raw("   "),
                        button("  Peer-to-peer  ", *peer_selected),
                    ]),
                    Line::from(""),
                    Line::from(Span::raw("←/→ choose · Enter confirm · Esc cancel")),
                ];
                draw_box(area, buf, " Push method ", body, 60, 9);
            }
            P2pModal::TicketInput { value } => {
                let count = value.trim().len();
                let tail: String = value
                    .trim()
                    .chars()
                    .rev()
                    .take(46)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let shown = if count > 46 { format!("…{tail}") } else { tail };
                let body = vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "Paste a peer-to-peer ticket:",
                        Style::default().bold(),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        format!("> {shown}"),
                        Style::default().fg(crate::theme::palette().emphasis),
                    )),
                    Line::from(Span::raw(format!("  ({count} chars)"))),
                    Line::from(""),
                    Line::from(Span::raw("Enter pull · Esc cancel")),
                ];
                draw_box(area, buf, " Receive a tour ", body, 64, 10);
            }
            P2pModal::ServingMenu { index } => {
                let name = self.p2p_serving.as_ref().map(|s| s.name.as_str()).unwrap_or("");
                let ticket = self
                    .p2p_serving
                    .as_ref()
                    .and_then(|s| s.ticket.as_deref())
                    .unwrap_or("(minting…)");
                let body = vec![
                    Line::from(""),
                    Line::from(Span::styled(format!("Serving '{name}'"), Style::default().bold())),
                    Line::from(Span::raw(ticket.to_string())),
                    Line::from(""),
                    Line::from(vec![
                        button("  Re-copy ticket  ", *index == 0),
                        Span::raw("  "),
                        button("  Stop serving  ", *index == 1),
                        Span::raw("  "),
                        button("  Close  ", *index == 2),
                    ]),
                    Line::from(""),
                    Line::from(Span::raw("←/→ choose · Enter · Esc close")),
                ];
                draw_box(area, buf, " Serving ", body, 72, 10);
            }
        }
    }
}

/// A modal button span: filled when selected, outlined otherwise.
fn button(label: &str, selected: bool) -> Span<'_> {
    let palette = crate::theme::palette();
    if selected {
        Span::styled(
            label.to_string(),
            Style::default().bg(palette.accent).fg(palette.inverse).bold(),
        )
    } else {
        Span::styled(label.to_string(), Style::default().fg(palette.emphasis))
    }
}

/// Draw a centered, bordered box with the given title and body lines.
fn draw_box(
    area: Rect,
    buf: &mut Buffer,
    title: &str,
    body: Vec<Line>,
    max_width: u16,
    height: u16,
) {
    let width = (area.width as f64 * 0.7).min(max_width as f64) as u16;
    let height = area.height.min(height);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    Widget::render(Clear, rect, buf);
    let palette = crate::theme::palette();
    Paragraph::new(Text::from(body))
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(title.to_string())
                .title_style(Style::default().bold())
                .border_style(Style::default().fg(palette.accent)),
        )
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false })
        .render(rect, buf);
}

#[cfg(test)]
mod tests {
    use super::looks_like_ticket;

    #[test]
    fn recognizes_ticket_shapes() {
        assert!(looks_like_ticket("blobabcdef0123456789"));
        assert!(looks_like_ticket("  blobabcdef0123456789\n"));
        assert!(!looks_like_ticket("blob"));
        assert!(!looks_like_ticket("https://example.com/tours/123"));
        assert!(!looks_like_ticket("not a ticket at all"));
        assert!(!looks_like_ticket("blob with spaces xxxxxxxx"));
    }
}
