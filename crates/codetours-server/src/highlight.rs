use quick_cache::sync::Cache;
use std::sync::{Arc, OnceLock};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::{IncludeBackground, styled_line_to_highlighted_html};
use syntect::parsing::SyntaxSet;
use xxhash_rust::xxh3::xxh3_64;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();
static HL_CACHE: OnceLock<Cache<(String, u64), Arc<String>>> = OnceLock::new();

pub fn get_syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

pub fn get_theme() -> &'static Theme {
    THEME.get_or_init(|| {
        let theme_bytes = include_bytes!("../static/AtomOneDark.tmTheme");
        let mut reader = std::io::Cursor::new(theme_bytes);
        ThemeSet::load_from_reader(&mut reader).expect("Failed to load embedded theme")
    })
}

pub fn get_cache() -> &'static Cache<(String, u64), Arc<String>> {
    HL_CACHE.get_or_init(|| Cache::new(10_000))
}

pub fn highlight(language: &str, content: &str) -> Arc<String> {
    let hash = xxh3_64(content.as_bytes());
    let key = (language.to_string(), hash);

    let cache = get_cache();
    if let Some(cached) = cache.get(&key) {
        return cached;
    }

    let syntax_set = get_syntax_set();
    let theme = get_theme();

    let syntax = syntax_set
        .find_syntax_by_extension(language)
        .or_else(|| syntax_set.find_syntax_by_token(language))
        .or_else(|| syntax_set.find_syntax_by_name(language))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut html = String::new();

    for line in content.lines() {
        let ranges = match highlighter.highlight_line(line, syntax_set) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Failed to highlight line for language '{}': {}", language, e);
                // Fall back to escaped plain text
                for c in line.chars() {
                    match c {
                        '&' => html.push_str("&amp;"),
                        '<' => html.push_str("&lt;"),
                        '>' => html.push_str("&gt;"),
                        '"' => html.push_str("&quot;"),
                        '\'' => html.push_str("&apos;"),
                        _ => html.push(c),
                    }
                }
                html.push('\n');
                continue;
            }
        };
        let escaped_html = match styled_line_to_highlighted_html(&ranges, IncludeBackground::No) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("Failed to format highlighted HTML for language '{}': {}", language, e);
                // Fall back to escaped plain text
                for c in line.chars() {
                    match c {
                        '&' => html.push_str("&amp;"),
                        '<' => html.push_str("&lt;"),
                        '>' => html.push_str("&gt;"),
                        '"' => html.push_str("&quot;"),
                        '\'' => html.push_str("&apos;"),
                        _ => html.push(c),
                    }
                }
                html.push('\n');
                continue;
            }
        };
        html.push_str(&escaped_html);
        html.push('\n');
    }

    let result = Arc::new(html);
    cache.insert(key, result.clone());
    result
}
