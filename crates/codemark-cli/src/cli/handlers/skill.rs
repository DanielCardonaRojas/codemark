//! `install-skill` handler: download and install the codemark agent skill
//! into the directory expected by a chosen AI coding agent.
//!
//! The skill is published as a release asset
//! (`codemark-skill-<version>.zip`) on the codemark GitHub repository. This
//! handler downloads the asset matching the running binary's version (or an
//! explicit `--version`), extracts it, and installs it into the agent/scope
//! specific directory.

use std::io::{self, Cursor, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::cli::output::OutputMode;
use crate::cli::{Cli, InstallSkillArgs, SkillAgent, SkillScope};
use codemark_core::error::{Error, Result};
use codemark_core::sync::build_sync_http_client;

const SKILL_NAME: &str = "codemark";
const REPO_SLUG: &str = "DanielCardonaRojas/codemark";

/// Handle `codemark install-skill`.
pub async fn handle_install_skill(
    _cli: &Cli,
    mode: &OutputMode,
    args: &InstallSkillArgs,
) -> Result<()> {
    let version = args.version.clone().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

    let asset_url = format!(
        "https://github.com/{REPO_SLUG}/releases/download/{version}/codemark-skill-{version}.zip"
    );

    // Resolve the directory that will contain the `codemark/` skill folder.
    let skills_root = skill_install_root(args.agent, args.scope)?;
    let target_dir = skills_root.join(SKILL_NAME);

    tracing::debug!(
        target: "codemark::skill",
        version = %version,
        agent = ?args.agent,
        scope = ?args.scope,
        target = %target_dir.display(),
        "installing skill"
    );

    // Confirm before clobbering an existing install.
    if target_dir.exists() && !args.force && !confirm_overwrite(mode, &target_dir)? {
        return Err(Error::Operation(format!(
            "skill already installed at {}; re-run with --force to overwrite",
            target_dir.display()
        )));
    }

    // Download the release asset.
    tracing::debug!(target: "codemark::http", url = %asset_url, "downloading skill asset");
    let client = build_sync_http_client()?;
    let response = client
        .get(&asset_url)
        .send()
        .await
        .map_err(|e| Error::Operation(format!("failed to download skill asset: {e}")))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(Error::Input(format!(
            "skill asset not published for version {version} — the maintainer may not have attached it to the {version} release"
        )));
    }
    if !response.status().is_success() {
        return Err(Error::Operation(format!(
            "failed to download skill asset: HTTP {}",
            response.status()
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| Error::Operation(format!("failed to read skill asset body: {e}")))?;

    // If overwriting, clear the previous install so removed files don't linger.
    if target_dir.exists() {
        std::fs::remove_dir_all(&target_dir)
            .map_err(|e| Error::Operation(format!("failed to remove existing skill: {e}")))?;
    }

    // Extract the archive into the skills root. The archive's top-level entry is
    // `codemark/` (produced by zipping the `codemark` folder), so files land
    // directly under `<skills_root>/codemark/`.
    std::fs::create_dir_all(&skills_root)
        .map_err(|e| Error::Operation(format!("failed to create install directory: {e}")))?;
    extract_skill_archive(&bytes, &skills_root)?;

    tracing::debug!(target: "codemark::skill", target = %target_dir.display(), "skill extracted");

    // Gemini consumes `.toml` custom commands rather than SKILL.md skills, so
    // drop a thin command file that points the agent at the installed SKILL.md.
    if matches!(args.agent, SkillAgent::Gemini) {
        write_gemini_shim(&skills_root, &target_dir)?;
    }

    report_success(mode, args.agent, &version, &target_dir)
}

/// Resolve the skills *root* directory (the parent that will contain the
/// `codemark/` skill folder) for a given agent and scope.
fn skill_install_root(agent: SkillAgent, scope: SkillScope) -> Result<PathBuf> {
    let base = match scope {
        SkillScope::User => codemark_core::config::home_dir()
            .ok_or_else(|| Error::Operation("could not determine home directory".to_string()))?,
        SkillScope::Project => std::env::current_dir()
            .map_err(|e| Error::Operation(format!("could not determine current directory: {e}")))?,
    };

    let rel: &str = match (agent, scope) {
        (SkillAgent::Claude, _) => ".claude/skills",
        (SkillAgent::Copilot, SkillScope::User) => ".copilot/skills",
        (SkillAgent::Copilot, SkillScope::Project) => ".github/skills",
        (SkillAgent::Agents, _) => ".agents/skills",
        (SkillAgent::Gemini, _) => ".gemini/skills",
    };

    Ok(base.join(rel))
}

/// Extract a zip archive into `dest`.
fn extract_skill_archive(bytes: &[u8], dest: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| Error::Operation(format!("failed to open skill archive: {e}")))?;
    archive
        .extract(dest)
        .map_err(|e| Error::Operation(format!("failed to extract skill archive: {e}")))?;
    Ok(())
}

/// Write a Gemini CLI custom command that delegates to the installed SKILL.md.
fn write_gemini_shim(skills_root: &Path, skill_dir: &Path) -> Result<()> {
    // skills_root is `<base>/.gemini/skills`; commands live in `<base>/.gemini/commands`.
    let Some(gemini_root) = skills_root.parent() else {
        return Ok(());
    };
    let commands_dir = gemini_root.join("commands");
    std::fs::create_dir_all(&commands_dir)
        .map_err(|e| Error::Operation(format!("failed to create Gemini commands dir: {e}")))?;

    let skill_md = skill_dir.join("SKILL.md");
    let toml = format!(
        "description = \"Manage structural code bookmarks with codemark\"\nprompt = \"\"\"\nFollow the instructions in the codemark skill at {} to fulfill the user's request.\n\"\"\"\n",
        skill_md.display()
    );
    let command_path = commands_dir.join(format!("{SKILL_NAME}.toml"));
    std::fs::write(&command_path, toml)
        .map_err(|e| Error::Operation(format!("failed to write Gemini command: {e}")))?;

    tracing::debug!(
        target: "codemark::skill",
        command = %command_path.display(),
        "wrote Gemini command shim"
    );
    Ok(())
}

/// Prompt the user to confirm overwriting an existing install.
///
/// In JSON mode or when stdin is not a TTY, refuse rather than block.
fn confirm_overwrite(mode: &OutputMode, target: &Path) -> Result<bool> {
    if matches!(mode, OutputMode::Json) || !io::stdin().is_terminal() {
        return Ok(false);
    }

    print!("Skill already installed at {}. Overwrite? [y/N] ", target.display());
    io::stdout().flush().ok();
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|e| Error::Operation(format!("failed to read confirmation: {e}")))?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Print a success message in the active output mode.
fn report_success(
    mode: &OutputMode,
    agent: SkillAgent,
    version: &str,
    target: &Path,
) -> Result<()> {
    let agent_name = match agent {
        SkillAgent::Claude => "claude",
        SkillAgent::Copilot => "copilot",
        SkillAgent::Agents => "agents",
        SkillAgent::Gemini => "gemini",
    };

    match mode {
        OutputMode::Json => {
            crate::cli::output::write_json_success(&serde_json::json!({
                "message": format!("Installed codemark skill {version} for {agent_name}"),
                "agent": agent_name,
                "version": version,
                "path": target.display().to_string(),
            }))?;
        }
        _ => {
            println!("Installed codemark skill {version} for {agent_name} at {}", target.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_install_root_paths() {
        let home = codemark_core::config::home_dir().unwrap();
        let cwd = std::env::current_dir().unwrap();

        let cases = [
            (SkillAgent::Claude, SkillScope::User, home.join(".claude/skills")),
            (SkillAgent::Claude, SkillScope::Project, cwd.join(".claude/skills")),
            (SkillAgent::Copilot, SkillScope::User, home.join(".copilot/skills")),
            (SkillAgent::Copilot, SkillScope::Project, cwd.join(".github/skills")),
            (SkillAgent::Agents, SkillScope::User, home.join(".agents/skills")),
            (SkillAgent::Agents, SkillScope::Project, cwd.join(".agents/skills")),
            (SkillAgent::Gemini, SkillScope::User, home.join(".gemini/skills")),
            (SkillAgent::Gemini, SkillScope::Project, cwd.join(".gemini/skills")),
        ];

        for (agent, scope, expected) in cases {
            assert_eq!(skill_install_root(agent, scope).unwrap(), expected);
        }
    }
}
