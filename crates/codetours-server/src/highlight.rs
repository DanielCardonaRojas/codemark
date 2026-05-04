use quick_cache::sync::Cache;
use std::sync::{Arc, OnceLock};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::{styled_line_to_highlighted_html, IncludeBackground};
use syntect::parsing::SyntaxSet;
use xxhash_rust::xxh3::xxh3_64;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();
static HL_CACHE: OnceLock<Cache<(String, u64), Arc<String>>> = OnceLock::new();

pub fn get_syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(|| SyntaxSet::load_defaults_newlines())
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
        .or_else(|| syntax_set.find_syntax_by_name(language))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut html = String::new();

    for line in content.lines() {
        let ranges = highlighter.highlight_line(line, syntax_set).unwrap_or_default();
        let escaped_html = styled_line_to_highlighted_html(&ranges, IncludeBackground::No).unwrap_or_default();
        html.push_str(&escaped_html);
        html.push('\n');
    }

    let result = Arc::new(html);
    cache.insert(key, result.clone());
    result
}
