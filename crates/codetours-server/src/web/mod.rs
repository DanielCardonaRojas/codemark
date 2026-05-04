pub mod negotiation;
pub mod handlers;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavItem {
    Tours,
    MyTours,
    Config,
    None,
}

pub mod filters {
    pub fn tag_color_classes(tag: &str) -> rinja::Result<String> {
        let classes = match tag.to_lowercase().as_str() {
            "bug" | "fix" => "bg-red-900/40 text-red-300",
            "feature" | "new" => "bg-green-900/40 text-green-300",
            "refactor" => "bg-blue-900/40 text-blue-300",
            "docs" => "bg-yellow-900/40 text-yellow-300",
            "test" => "bg-purple-900/40 text-purple-300",
            _ => "bg-bg-tertiary text-text-secondary",
        };
        Ok(classes.to_string())
    }
}
