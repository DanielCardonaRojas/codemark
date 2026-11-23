//! A ratatui [`Backend`] wrapper that writes nothing for a frame that changed
//! nothing.
//!
//! The TUI redraws on every 100ms tick. ratatui only sends the cells that
//! changed, but `CrosstermBackend::draw` still ends every frame with a style
//! reset, and `Terminal::draw` re-sends hide-cursor (or show-cursor plus a
//! cursor move) on every frame. An idle TUI therefore wrote ~25 bytes per tick
//! forever.
//!
//! That trickle is what froze the TUI after switching tabs. macOS ptys buffer
//! only 1 KB each way. A terminal that stops draining a hidden tab lets the
//! buffer fill in about four seconds, and the UI thread then blocks in
//! `write()`. While it is blocked, the input reader can't deliver anything
//! (Ctrl+C included). If the terminal in turn blocks writing input back to us,
//! neither side can make progress and the tab has to be closed.
//!
//! This wrapper drops those no-op writes: an empty diff emits nothing, and
//! cursor visibility/position commands are only sent when they change.
//! An idle TUI writes zero bytes, so a hidden tab never fills the pty.

use std::io;

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

/// Wraps a [`Backend`], suppressing frames and cursor commands that would not
/// change what the terminal shows.
#[derive(Debug)]
pub struct QuietBackend<B: Backend> {
    inner: B,
    /// Last cursor visibility sent; `None` until the first command, so the
    /// initial state is always written.
    cursor_visible: Option<bool>,
    /// Where the cursor was last placed; `None` once anything else moved it
    /// (drawing cells, clearing) so the next placement is always written.
    cursor_position: Option<Position>,
}

impl<B: Backend> QuietBackend<B> {
    pub fn new(inner: B) -> Self {
        Self { inner, cursor_visible: None, cursor_position: None }
    }

    /// The wrapped backend.
    pub fn inner(&self) -> &B {
        &self.inner
    }

    /// The wrapped backend, mutably. Writing through it bypasses the cursor
    /// tracking, so the tracked state is reset to "unknown".
    pub fn inner_mut(&mut self) -> &mut B {
        self.cursor_visible = None;
        self.cursor_position = None;
        &mut self.inner
    }
}

impl<B: Backend> Backend for QuietBackend<B> {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut content = content.peekable();
        if content.peek().is_none() {
            // Nothing changed: skip the backend's trailing style reset.
            return Ok(());
        }
        // Drawing moves the terminal's cursor to each written cell.
        self.cursor_position = None;
        self.inner.draw(content)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.cursor_position = None;
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        if self.cursor_visible == Some(false) {
            return Ok(());
        }
        self.inner.hide_cursor()?;
        self.cursor_visible = Some(false);
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        if self.cursor_visible == Some(true) {
            return Ok(());
        }
        self.inner.show_cursor()?;
        self.cursor_visible = Some(true);
        Ok(())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        if self.cursor_position == Some(position) {
            return Ok(());
        }
        self.inner.set_cursor_position(position)?;
        self.cursor_position = Some(position);
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        self.cursor_position = None;
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.cursor_position = None;
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::widgets::Paragraph;
    use ratatui::{Terminal, TerminalOptions, Viewport};
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// A writer whose bytes the test can read back after the backend took it.
    #[derive(Clone, Default)]
    struct Sink(Rc<RefCell<Vec<u8>>>);

    impl io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    type TestTerminal = Terminal<QuietBackend<CrosstermBackend<Sink>>>;

    fn terminal() -> (TestTerminal, Sink) {
        let sink = Sink::default();
        let terminal = Terminal::with_options(
            QuietBackend::new(CrosstermBackend::new(sink.clone())),
            TerminalOptions { viewport: Viewport::Fixed(Rect::new(0, 0, 20, 3)) },
        )
        .expect("terminal");
        (terminal, sink)
    }

    /// Bytes written since the last call.
    fn take_written(sink: &Sink) -> Vec<u8> {
        std::mem::take(&mut *sink.0.borrow_mut())
    }

    #[test]
    fn an_unchanged_frame_writes_nothing() {
        let (mut terminal, sink) = terminal();
        let draw = |t: &mut Terminal<_>, text: &'static str| {
            t.draw(|f| f.render_widget(Paragraph::new(text), f.area())).expect("draw");
        };

        draw(&mut terminal, "hello");
        assert!(!take_written(&sink).is_empty(), "the first frame paints");

        // The idle tick redraw: same content, no cursor.
        draw(&mut terminal, "hello");
        draw(&mut terminal, "hello");
        assert!(take_written(&sink).is_empty(), "an idle redraw must not write");

        draw(&mut terminal, "world");
        assert!(!take_written(&sink).is_empty(), "a changed frame still paints");
    }

    #[test]
    fn a_still_cursor_is_written_once() {
        let (mut terminal, sink) = terminal();
        let draw = |t: &mut Terminal<_>, x: u16| {
            t.draw(|f| {
                f.render_widget(Paragraph::new("text"), f.area());
                f.set_cursor_position((x, 0));
            })
            .expect("draw");
        };

        draw(&mut terminal, 2);
        assert!(!take_written(&sink).is_empty());

        draw(&mut terminal, 2);
        assert!(take_written(&sink).is_empty(), "same cursor, same frame: no bytes");

        draw(&mut terminal, 3);
        assert!(!take_written(&sink).is_empty(), "a moved cursor is written");
    }

    #[test]
    fn the_cursor_is_replaced_after_cells_are_drawn() {
        let (mut terminal, sink) = terminal();
        let draw = |t: &mut Terminal<_>, text: &'static str| {
            t.draw(|f| {
                f.render_widget(Paragraph::new(text), f.area());
                f.set_cursor_position((1, 1));
            })
            .expect("draw");
        };

        draw(&mut terminal, "a");
        take_written(&sink);
        // Painting the changed cell moves the real cursor, so the placement at
        // (1, 1) must be re-sent even though it didn't change.
        draw(&mut terminal, "b");
        let written = String::from_utf8_lossy(&take_written(&sink)).into_owned();
        assert!(written.ends_with("\u{1b}[2;2H"), "cursor re-placed last: {written:?}");
    }
}
