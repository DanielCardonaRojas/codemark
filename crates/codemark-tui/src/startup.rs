//! Startup options for the TUI: open a specific collection and/or start in a
//! named pane layout.
//!
//! The TUI is launched both as the standalone `codemark-tui` binary and as the
//! bundled `codemark tui` subcommand, where the CLI forwards trailing arguments
//! unparsed. Parsing here is deliberately tolerant of leading positional words
//! (`codemark`, `tui`) and only acts on the flags it knows about, mirroring how
//! [`crate::entry::run`] already inspects `--list-themes`.
//!
//! ```text
//! codemark tui --collection auth-flow --layout code
//! codemark-tui -c auth-flow -l preview
//! ```

use std::fmt;
use std::str::FromStr;

/// A named arrangement of the browser panes to start in.
///
/// Each preset maps onto the existing resize states the `+`/`-` keys cycle
/// through, so a startup layout is always one the user could reach by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartupLayout {
    /// The regular split: sidebar on the left, preview on the right.
    #[default]
    Default,
    /// The content list (bookmarks/collections) takes the full width.
    List,
    /// The preview (code, pager and details) takes the full width.
    Preview,
    /// Only the code preview and pager, full width — no sidebar or details.
    Code,
    /// The details pane (notes, comments) takes the full screen.
    Details,
}

impl StartupLayout {
    /// Every layout, in the order they are listed in `--help`.
    pub const ALL: [StartupLayout; 5] =
        [Self::Default, Self::List, Self::Preview, Self::Code, Self::Details];

    /// The name accepted by `--layout`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::List => "list",
            Self::Preview => "preview",
            Self::Code => "code",
            Self::Details => "details",
        }
    }

    /// One-line description shown in `--help`.
    pub fn description(self) -> &'static str {
        match self {
            Self::Default => "sidebar and preview side by side",
            Self::List => "content list at full width",
            Self::Preview => "code, pager and details at full width",
            Self::Code => "code and pager only, at full width",
            Self::Details => "details pane (notes, comments) full screen",
        }
    }
}

impl fmt::Display for StartupLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for StartupLayout {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let wanted = s.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|l| l.name() == wanted).ok_or_else(|| {
            let names: Vec<&str> = Self::ALL.iter().map(|l| l.name()).collect();
            format!("unknown layout '{s}' (expected one of: {})", names.join(", "))
        })
    }
}

/// Options that shape the initial state of the dashboard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupOptions {
    /// Name of a collection to open (enter its steps) on launch.
    pub collection: Option<String>,
    /// Pane layout to start in. `None` keeps the default behaviour.
    pub layout: Option<StartupLayout>,
    /// `--help` / `-h` was passed: print usage and exit.
    pub help: bool,
}

impl StartupOptions {
    /// Parse startup flags from raw process arguments.
    ///
    /// Accepts `--collection <name>`, `--collection=<name>`, `-c <name>`, and
    /// the same forms for `--layout`/`-l`. Unknown arguments (including the
    /// program name and the `tui` subcommand word) are ignored so other flags
    /// such as `--list-themes` keep working.
    pub fn parse<I, S>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let args: Vec<String> = args.into_iter().map(|a| a.as_ref().to_string()).collect();
        let mut opts = Self::default();
        let mut i = 0;
        while i < args.len() {
            let arg = args[i].as_str();
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
                _ => (arg, None),
            };
            match flag {
                "--help" | "-h" => opts.help = true,
                "--collection" | "-c" | "--layout" | "-l" => {
                    let value = match inline {
                        Some(v) => v,
                        None => {
                            i += 1;
                            args.get(i)
                                .filter(|v| !v.starts_with('-'))
                                .cloned()
                                .ok_or_else(|| format!("{flag} requires a value"))?
                        }
                    };
                    if value.trim().is_empty() {
                        return Err(format!("{flag} requires a value"));
                    }
                    if flag == "--collection" || flag == "-c" {
                        opts.collection = Some(value);
                    } else {
                        opts.layout = Some(value.parse()?);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        Ok(opts)
    }

    /// Usage text printed for `--help`.
    pub fn usage() -> String {
        let mut out = String::from(
            "Codemark dashboard\n\n\
             Usage: codemark tui [OPTIONS]\n\n\
             Options:\n  \
             -c, --collection <NAME>  Open the named collection on launch\n  \
             -l, --layout <LAYOUT>    Start in a pane layout (see below)\n      \
             --list-themes        Print every theme name and exit\n      \
             --list-schemes       Print base16/base24 scheme names and exit\n  \
             -h, --help               Print this help\n\n\
             Layouts:\n",
        );
        for layout in StartupLayout::ALL {
            out.push_str(&format!("  {:<10} {}\n", layout.name(), layout.description()));
        }
        out.push_str("\nExample: codemark tui --collection auth-flow --layout code\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_flags_yields_defaults() {
        let opts = StartupOptions::parse(["codemark", "tui"]).unwrap();
        assert_eq!(opts, StartupOptions::default());
    }

    #[test]
    fn parses_long_short_and_inline_forms() {
        let opts = StartupOptions::parse(["codemark", "tui", "--collection", "auth", "-l", "code"])
            .unwrap();
        assert_eq!(opts.collection.as_deref(), Some("auth"));
        assert_eq!(opts.layout, Some(StartupLayout::Code));

        let opts =
            StartupOptions::parse(["codemark-tui", "--collection=my tour", "--layout=Preview"])
                .unwrap();
        assert_eq!(opts.collection.as_deref(), Some("my tour"));
        assert_eq!(opts.layout, Some(StartupLayout::Preview));

        let opts = StartupOptions::parse(["codemark-tui", "-c", "x"]).unwrap();
        assert_eq!(opts.collection.as_deref(), Some("x"));
        assert_eq!(opts.layout, None);
    }

    #[test]
    fn collection_named_tui_is_not_confused_with_subcommand() {
        let opts = StartupOptions::parse(["codemark", "tui", "-c", "tui"]).unwrap();
        assert_eq!(opts.collection.as_deref(), Some("tui"));
    }

    #[test]
    fn missing_value_is_an_error() {
        assert!(StartupOptions::parse(["codemark-tui", "--collection"]).is_err());
        assert!(StartupOptions::parse(["codemark-tui", "-c", "--layout", "code"]).is_err());
        assert!(StartupOptions::parse(["codemark-tui", "--layout="]).is_err());
    }

    #[test]
    fn unknown_layout_lists_valid_names() {
        let err = StartupOptions::parse(["codemark-tui", "--layout", "wide"]).unwrap_err();
        assert!(err.contains("wide"));
        for layout in StartupLayout::ALL {
            assert!(err.contains(layout.name()));
        }
    }

    #[test]
    fn other_flags_are_ignored_and_help_is_detected() {
        let opts = StartupOptions::parse(["codemark-tui", "--list-themes", "-h"]).unwrap();
        assert!(opts.help);
    }

    #[test]
    fn every_layout_round_trips_through_its_name() {
        for layout in StartupLayout::ALL {
            assert_eq!(layout.name().parse::<StartupLayout>().unwrap(), layout);
        }
    }
}
