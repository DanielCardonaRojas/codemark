# Plan: Migrating `MarkdownPanel` to `pulldown-cmark`

This plan outlines the steps required to replace the brittle custom string-manipulation markdown parser in `codemark-tui` with the robust, event-driven `pulldown-cmark` library.

## 1. Dependency Management
- **Remove:** `tui-markdown` from `crates/codemark-tui/Cargo.toml` as it was included accidentally and is currently unused.
- **Add:** `pulldown-cmark` to `crates/codemark-tui/Cargo.toml`.
  ```toml
  pulldown-cmark = { version = "0.12" } # or current stable version
  ```

## 2. Refactoring `MarkdownPanel` Parsing Logic
The core change involves rewriting `MarkdownPanel::parse_to_text()` and removing the manual `parse_inline()`.

### From Line-by-Line to Event Stream
Instead of `self.content.lines()`, we will instantiate a `pulldown_cmark::Parser` and iterate through its event stream (`Event::Start`, `Event::End`, `Event::Text`, `Event::Code`, etc.).

### State Management During Parsing
Since `pulldown-cmark` emits events linearly, we need to maintain state as we build the Ratatui `Text` object:
- `lines: Vec<Line<'static>>`: The final list of Ratatui lines.
- `current_spans: Vec<Span<'static>>`: Spans for the line currently being built.
- `style_stack: Vec<Style>`: A stack to keep track of nested styles. As we encounter `Start(Tag)`, we push a new `Style` (derived from merging the new style with the top of the stack). On `End(Tag)`, we pop from the stack.
- `list_depth` and `quote_depth`: To handle visual prefixes like `• ` and `┃ `.

## 3. Mapping Markdown Tags to Ratatui Styles
We will map `pulldown-cmark` tags to the existing `crate::theme::palette()` styles to maintain visual parity:

| Markdown Element | `pulldown-cmark` Event | Target Ratatui Style / Formatting |
| :--- | :--- | :--- |
| **H1** | `Start(Heading(H1))` | `palette().warning` + `BOLD` + `UNDERLINED` |
| **H2** | `Start(Heading(H2))` | `palette().accent` + `BOLD` |
| **Blockquote** | `Start(BlockQuote)` | Prefix `┃ ` (dim) + text in `gray` + `ITALIC` |
| **List Item** | `Start(Item)` | Prefix `• ` (`accent` color) |
| **Inline Code** | `Code(text)` | `palette().warning` |
| **Bold** | `Start(Strong)` | Add `BOLD` modifier |
| **Italic** | `Start(Emphasis)` | Add `ITALIC` modifier |
| **Text** | `Text(text)` | Use the style currently at the top of `style_stack` |
| **Soft/Hard Break** | `SoftBreak` / `HardBreak` | Push `current_spans` to `lines`, start a new empty line |

## 4. Handling Tables
Currently, the custom parser handles tables using a simplistic `|` split, coloring the first column `dim` and the rest normally.
- `pulldown-cmark` emits `Table`, `TableHead`, `TableRow`, and `TableCell` tags.
- We will need to decide whether to implement a simple column width calculator to align table cells properly or just pad the cells manually using a fixed width (like the current `format!("{:<15}", key)` implementation).

## 5. Cleaning up Tests
The tests in `crates/codemark-tui/src/component/markdown_panel.rs` (like `backslash_escapes_are_consumed` and `escaped_formatting_chars_are_literal`) were written to verify the custom escape logic. 
- These will need to be updated. Since `pulldown-cmark` correctly handles standard Markdown escaping (`\_`, `\*`), the custom logic can be deleted, but we should keep/adapt the tests to ensure the overall pipeline (Markdown String -> Ratatui Text) still honors those escapes.

## 6. Execution Phases
1. **Setup:** Update Cargo.toml dependencies.
2. **Implementation:** Write the event stream loop in `parse_to_text`.
3. **Styling:** Hook up the `style_stack` and map the basic tags (Headings, Bold, Code, Lists).
4. **Tables & Edge Cases:** Implement table row parsing and blockquote prefixes.
5. **Testing:** Run the TUI and verify bookmarks render correctly without regression, and fix the existing unit tests.

## 7. Files to Modify
1. **`crates/codemark-tui/Cargo.toml`**
   - Remove `tui-markdown = "0.3.7"`
   - Add `pulldown-cmark = "0.12"` (or the latest stable version)
2. **`crates/codemark-tui/src/component/markdown_panel.rs`**
   - Remove `fn parse_inline(...)` completely.
   - Remove `fn flush(...)` completely.
   - Completely rewrite `fn parse_to_text(&self) -> Text<'static>`.
   - Update `mod tests` block to test against standard Markdown escape logic instead of the custom string scanner.

## 8. Detailed Implementation Hints

### 1. State Tracking Architecture
Inside the new `parse_to_text`, you will want to track the current styling context and line being built:
```rust
use pulldown_cmark::{Event, Parser, Tag};

let mut lines = Vec::new();
let mut current_spans = Vec::new();
// Maintain a stack of styles. Text inherits the style at the top of the stack.
let mut style_stack = vec![Style::default()];

// Flags/counters for context-aware styling (like prefixes)
let mut in_blockquote = false;
let mut cell_index = 0;

for event in Parser::new(&self.content) {
    match event {
        // ... match events ...
    }
}
```

### 2. Pushing and Popping Styles
When encountering styling tags, merge them with the current style:
```rust
Event::Start(Tag::Strong) => {
    let current_style = *style_stack.last().unwrap();
    style_stack.push(current_style.add_modifier(Modifier::BOLD));
}
Event::End(Tag::Strong) => {
    style_stack.pop();
}
```

### 3. Emitting Text
When you get text or inline code, use the style at the top of the stack. Because `ratatui::text::Span` requires static strings, you'll need to allocate or use `Cow`:
```rust
Event::Text(text) => {
    let style = *style_stack.last().unwrap();
    current_spans.push(Span::styled(text.into_string(), style));
}
Event::Code(text) => {
    let style = *style_stack.last().unwrap();
    current_spans.push(Span::styled(
        text.into_string(), 
        style.fg(crate::theme::palette().warning)
    ));
}
```

### 4. Handling Blockquotes and Lists (Prefixes)
Because `pulldown-cmark` only tells you when a blockquote starts and ends, you must manually inject the `┃ ` or `• ` prefixes at the start of new lines:
```rust
Event::Start(Tag::BlockQuote) => {
    in_blockquote = true;
    // You might want to push an italicized/gray style to the stack here too
}
Event::End(Tag::BlockQuote) => {
    in_blockquote = false;
}
// When starting a new line (or handling SoftBreak/HardBreak):
Event::SoftBreak | Event::HardBreak => {
    lines.push(Line::from(std::mem::take(&mut current_spans)));
    if in_blockquote {
        current_spans.push(Span::styled("┃ ", Style::default().fg(crate::theme::palette().dim)));
    }
}
```

### 5. Table Layout Hack
The current codebase expects a simple `<Key> | <Value>` layout padded to 15 characters. You can emulate this using cell indexing:
```rust
Event::Start(Tag::TableRow) => {
    cell_index = 0;
}
Event::Start(Tag::TableCell) => {
    if cell_index == 0 {
        // We are in the "Key" column, push dim style
        style_stack.push(*style_stack.last().unwrap().fg(crate::theme::palette().dim));
    } else {
        style_stack.push(*style_stack.last().unwrap());
    }
}
Event::Text(text) if in_table_cell => {
    if cell_index == 0 {
        // Pad the key to 15 spaces
        let padded = format!("{:<15}", text.trim_matches('*'));
        current_spans.push(Span::styled(padded, *style_stack.last().unwrap()));
    } else {
        current_spans.push(Span::styled(text.into_string(), *style_stack.last().unwrap()));
    }
}
Event::End(Tag::TableCell) => {
    style_stack.pop();
    cell_index += 1;
}
```
