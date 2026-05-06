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
        let tag_lower = tag.to_lowercase();
        let classes = match tag_lower.as_str() {
            "rust" => "bg-orange-500/10 text-orange-500 border border-orange-500/20",
            "core" => "bg-blue-500/10 text-blue-500 border border-blue-500/20",
            "onboarding" => "bg-green-500/10 text-green-500 border border-green-500/20",
            "bug" | "fix" | "critical" => "bg-red-500/10 text-red-500 border border-red-500/20",
            "feat" | "feature" | "new" => "bg-emerald-500/10 text-emerald-500 border border-emerald-500/20",
            "docs" | "tutorial" => "bg-yellow-500/10 text-yellow-500 border border-yellow-500/20",
            _ => {
                // Deterministic color based on name hash
                let mut h = 0u64;
                for b in tag_lower.as_bytes() {
                    h = h.wrapping_add(*b as u64).wrapping_mul(0x517cc1b727220a95);
                }
                match h % 6 {
                    0 => "bg-blue-500/10 text-blue-400 border border-blue-500/20",
                    1 => "bg-purple-500/10 text-purple-400 border border-purple-500/20",
                    2 => "bg-pink-500/10 text-pink-400 border border-pink-500/20",
                    3 => "bg-indigo-500/10 text-indigo-400 border border-indigo-500/20",
                    4 => "bg-cyan-500/10 text-cyan-400 border border-cyan-500/20",
                    _ => "bg-teal-500/10 text-teal-400 border border-teal-500/20",
                }
            }
        };
        Ok(classes.to_string())
    }
}
