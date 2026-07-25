//! Path shortening used when rendering compressible path items.

/// Shorten a file path to fit within `max_width` display columns, prioritizing
/// the last path components.
///
/// If the path exceeds `max_width`, it is truncated to show the trailing
/// components with a `../` prefix. For example, `/very/long/path/to/file.rs`
/// with `max_width=18` becomes `../path/to/file.rs`.
pub(super) fn shorten_path(path: &str, max_width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    if path.width() <= max_width {
        return path.to_string();
    }

    // Try to find a good breaking point by working backwards from the end
    let components: Vec<&str> = path.split('/').collect();
    let mut result = String::new();
    let prefix_overhead = 3; // "../" prefix that will be added if we truncate

    // Start from the last component and work backwards. All widths are measured
    // in display columns so multi-byte paths aren't truncated prematurely.
    for component in components.iter().rev() {
        let component_width = component.width();
        let separator_width = if result.is_empty() { 0 } else { 1 }; // '/' separator

        // Use max_width - prefix_overhead unconditionally to avoid edge cases
        // where we build a string that then exceeds max_width after prepending "../"
        let budget = max_width.saturating_sub(prefix_overhead);
        if result.width() + component_width + separator_width > budget {
            // Stop here and add "../" prefix if we have any components
            if !result.is_empty() {
                result = format!("../{}", result);
            }
            break;
        }

        // Add this component
        if result.is_empty() {
            result = component.to_string();
        } else {
            result = format!("{}/{}", component, result);
        }
    }

    // Fallback: if we couldn't build anything meaningful, keep the last
    // `max_width` display columns with a leading ellipsis, walking back over
    // char boundaries so we never split a multi-byte character.
    if result.is_empty() || result.width() > max_width {
        if path.width() > max_width {
            let keep = max_width.saturating_sub(3); // room for the "..." prefix
            let mut width = 0;
            let mut start = path.len();
            for (idx, ch) in path.char_indices().rev() {
                let w = ch.width().unwrap_or(0);
                if width + w > keep {
                    break;
                }
                width += w;
                start = idx;
            }
            format!("...{}", &path[start..])
        } else {
            path.to_string()
        }
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shorten_path_short_path() {
        let path = "src/main.rs";
        assert_eq!(shorten_path(path, 25), "src/main.rs");
    }

    #[test]
    fn test_shorten_path_exact_length() {
        let path = "src/main.rs"; // 11 characters
        assert_eq!(shorten_path(path, 11), "src/main.rs");
    }

    #[test]
    fn test_shorten_path_long_path() {
        let path = "very/long/path/to/some/deeply/nested/file.rs";
        let result = shorten_path(path, 25);
        assert!(result.len() <= 25);
        assert!(result.ends_with("file.rs"));
    }

    #[test]
    fn test_shorten_path_absolute_path() {
        let path = "/Users/danielcardona/development/codemark/src/browser/tabbed_panel.rs";
        let result = shorten_path(path, 25);
        assert!(result.len() <= 25);
        assert!(result.contains("tabbed_panel.rs") || result.contains("..."));
    }

    #[test]
    fn test_shorten_path_very_long_filename() {
        let path = "src/very_long_filename_that_exceeds_limit.rs";
        let result = shorten_path(path, 25);
        assert!(result.len() <= 25);
    }

    #[test]
    fn test_shorten_path_multibyte_uses_display_width() {
        // A path that fits in display columns but exceeds byte length must not
        // be truncated (each CJK char is 2 columns but 3 bytes).
        let path = "文档/main.rs"; // 12 display columns, 14 bytes
        assert_eq!(shorten_path(path, 12), path);
    }
}
