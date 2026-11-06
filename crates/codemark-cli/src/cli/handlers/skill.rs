//! `install-skill` handler: download and install the codemark agent skill
//! into the directory expected by a chosen AI coding agent.
//!
//! The skill is published as a release asset
//! (`codemark-skill-<version>.zip`) on the codemark GitHub repository. This
//! handler downloads the asset matching the running binary's version (or an
//! explicit `--version`), extracts it, and installs it into the agent/scope
//! specific directory.
//!
//! Every install records the skill's release version in a `VERSION` file next
//! to `SKILL.md`, and `install-skill --check` compares those markers against
//! the running binary so a stale skill (binary upgraded, skill left behind) is
//! reported with the exact command that reinstalls it.

use std::io::{self, Cursor, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::cli::output::OutputMode;
use crate::cli::{Cli, InstallSkillArgs, SkillAgent, SkillScope};
use codemark_core::error::{Error, Result};
use codemark_core::sync::build_sync_http_client;

const SKILL_NAME: &str = "codemark";
const REPO_SLUG: &str = "DanielCardonaRojas/codemark";

/// File inside the installed skill folder that pins the release version the
/// skill was published for. Stamped by `release-skill.yml` and (re)written by
/// every `install-skill` so installs of older archives carry it too.
const VERSION_FILE: &str = "VERSION";

/// Version of the running binary.
const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");

const ALL_AGENTS: [SkillAgent; 4] =
    [SkillAgent::Claude, SkillAgent::Copilot, SkillAgent::Agents, SkillAgent::Gemini];
const ALL_SCOPES: [SkillScope; 2] = [SkillScope::User, SkillScope::Project];

/// Upper bound on the skill archive download size (16 MiB). The skill is a
/// handful of markdown files, so anything larger indicates a wrong/corrupt
/// asset and is rejected before buffering it into memory.
const MAX_ASSET_BYTES: u64 = 16 * 1024 * 1024;

/// Validate a release version/tag before interpolating it into a URL.
///
/// Tags are simple (`0.7.3`); anything with path separators, whitespace, or
/// URL-special characters is a typo and would produce a confusing HTTP error
/// rather than the friendly "asset not published" message.
fn validate_version(version: &str) -> Result<()> {
    let ok = !version.is_empty()
        && version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'));
    if ok {
        Ok(())
    } else {
        Err(Error::Input(format!(
            "invalid version '{version}': expected a release tag like '0.7.3'"
        )))
    }
}

/// Handle `codemark install-skill`.
pub async fn handle_install_skill(
    _cli: &Cli,
    mode: &OutputMode,
    args: &InstallSkillArgs,
) -> Result<()> {
    if args.check {
        return handle_check(mode, args.agent, args.scope);
    }
    // clap enforces both unless `--check` is set.
    let (Some(agent), Some(scope)) = (args.agent, args.scope) else {
        return Err(Error::Input("--agent and --scope are required to install".to_string()));
    };

    let version = args.version.clone().unwrap_or_else(|| BINARY_VERSION.to_string());
    validate_version(&version)?;

    let asset_url = format!(
        "https://github.com/{REPO_SLUG}/releases/download/{version}/codemark-skill-{version}.zip"
    );

    // Resolve the directory that will contain the `codemark/` skill folder.
    let skills_root = skill_install_root(agent, scope)?;
    let target_dir = skills_root.join(SKILL_NAME);

    tracing::debug!(
        target: "codemark::skill",
        version = %version,
        agent = ?agent,
        scope = ?scope,
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

    // Reject oversized assets before buffering them into memory. The skill is a
    // few markdown files, so a large content-length means a wrong/corrupt asset.
    if let Some(len) = response.content_length()
        && len > MAX_ASSET_BYTES
    {
        return Err(Error::Operation(format!(
            "skill asset is too large ({len} bytes, limit {MAX_ASSET_BYTES})"
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| Error::Operation(format!("failed to read skill asset body: {e}")))?;

    // Guard again in case content-length was absent or understated.
    if bytes.len() as u64 > MAX_ASSET_BYTES {
        return Err(Error::Operation(format!(
            "skill asset is too large ({} bytes, limit {MAX_ASSET_BYTES})",
            bytes.len()
        )));
    }

    // Extract into a temporary staging directory first, then swap it into place
    // atomically. This avoids destroying an existing install if extraction fails
    // partway through (corrupted zip, disk-full, I/O error, etc.).
    std::fs::create_dir_all(&skills_root)
        .map_err(|e| Error::Operation(format!("failed to create install directory: {e}")))?;

    let staging_root = skills_root.join(format!(".{SKILL_NAME}.tmp-{}", std::process::id()));
    // Clear any leftover staging dir from a previous interrupted run.
    let _ = std::fs::remove_dir_all(&staging_root);

    // The archive's top-level entry is `codemark/` (produced by zipping the
    // `codemark` folder), so files land directly under `<staging_root>/codemark/`.
    if let Err(e) = extract_skill_archive(&bytes, &staging_root) {
        let _ = std::fs::remove_dir_all(&staging_root);
        return Err(e);
    }
    let staged_skill = staging_root.join(SKILL_NAME);

    // Pin the installed version so `--check` can detect a mismatched binary
    // later. Archives published before version stamping lack the file, so it
    // is always written from the tag that was actually downloaded.
    if let Err(e) = record_installed_version(&staged_skill, &version) {
        let _ = std::fs::remove_dir_all(&staging_root);
        return Err(e);
    }

    // Swap the freshly extracted skill into place. Only now is the old install
    // removed, immediately before the rename, minimizing the failure window.
    if target_dir.exists() {
        std::fs::remove_dir_all(&target_dir).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staging_root);
            Error::Operation(format!("failed to remove existing skill: {e}"))
        })?;
    }
    if let Err(e) = std::fs::rename(&staged_skill, &target_dir) {
        let _ = std::fs::remove_dir_all(&staging_root);
        return Err(Error::Operation(format!("failed to install skill: {e}")));
    }
    let _ = std::fs::remove_dir_all(&staging_root);

    tracing::debug!(target: "codemark::skill", target = %target_dir.display(), "skill extracted");

    // Gemini consumes `.toml` custom commands rather than SKILL.md skills, so
    // drop a thin command file that points the agent at the installed SKILL.md.
    if matches!(agent, SkillAgent::Gemini) {
        write_gemini_shim(&skills_root, scope)?;
    }

    report_success(mode, agent, &version, &target_dir)
}

/// Write the `VERSION` marker into an extracted skill folder.
///
/// An archive stamped with a *different* release than the one requested was
/// attached to the wrong release; installing it under the requested tag would
/// make `--check` vouch for the wrong instructions, so it is rejected (before
/// any existing install is touched). Archives without a marker, or carrying the
/// unstamped `-dev` placeholder, take the requested tag.
fn record_installed_version(skill_dir: &Path, version: &str) -> Result<()> {
    let path = skill_dir.join(VERSION_FILE);
    if let Some(stamped) = read_version_file(skill_dir)
        && !is_dev_version(&stamped)
        && normalize_version(&stamped) != normalize_version(version)
    {
        tracing::warn!(
            target: "codemark::skill",
            stamped = %stamped,
            requested = %version,
            "skill archive is stamped with a different version than its release tag"
        );
        return Err(Error::Operation(format!(
            "skill archive for {version} is stamped as version {stamped}; refusing to install a mismatched skill"
        )));
    }
    std::fs::write(&path, format!("{version}\n"))
        .map_err(|e| Error::Operation(format!("failed to record skill version: {e}")))?;
    tracing::debug!(target: "codemark::skill", version = %version, path = %path.display(), "recorded skill version");
    Ok(())
}

/// Read the `VERSION` marker of an installed skill, if present and non-empty.
fn read_version_file(skill_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(skill_dir.join(VERSION_FILE)).ok()?;
    let v = raw.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// Read the `metadata.codemark-version` frontmatter field of a SKILL.md.
///
/// Fallback for installs whose `VERSION` file was removed; only the
/// frontmatter block (between the leading `---` fences) is scanned.
fn read_frontmatter_version(skill_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(skill_dir.join("SKILL.md")).ok()?;
    let mut lines = raw.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    lines
        .take_while(|l| l.trim() != "---")
        .find_map(|l| l.trim().strip_prefix("codemark-version:"))
        .map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        .filter(|v| !v.is_empty())
}

/// The version an installed skill was released for, if it records one.
fn installed_skill_version(skill_dir: &Path) -> Option<String> {
    read_version_file(skill_dir).or_else(|| read_frontmatter_version(skill_dir))
}

/// Strip a leading `v` so `v0.8.0` and `0.8.0` compare equal.
fn normalize_version(v: &str) -> &str {
    v.strip_prefix('v').unwrap_or(v)
}

/// Development builds (`0.0.0-dev` placeholder in the repo copy) never warn.
fn is_dev_version(v: &str) -> bool {
    v.ends_with("-dev")
}

/// How an installed skill relates to the running binary.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SkillStatus {
    /// Skill version equals the binary version.
    Current,
    /// Skill was released for a different version.
    Mismatch,
    /// Skill predates version pinning, so its version is unknown.
    Unversioned,
    /// Either side is a development build; comparison skipped.
    Dev,
}

impl SkillStatus {
    fn as_str(&self) -> &'static str {
        match self {
            SkillStatus::Current => "current",
            SkillStatus::Mismatch => "mismatch",
            SkillStatus::Unversioned => "unversioned",
            SkillStatus::Dev => "dev",
        }
    }

    fn needs_reinstall(&self) -> bool {
        matches!(self, SkillStatus::Mismatch | SkillStatus::Unversioned)
    }
}

/// Compare an installed skill's version against the binary's.
///
/// Matching is exact (modulo a leading `v`): patch releases do change the
/// skill's instructions, and reinstalling is a single command.
fn compare_versions(skill: Option<&str>, binary: &str) -> SkillStatus {
    match skill {
        None => SkillStatus::Unversioned,
        Some(s) if is_dev_version(s) || is_dev_version(binary) => SkillStatus::Dev,
        Some(s) if normalize_version(s) == normalize_version(binary) => SkillStatus::Current,
        Some(_) => SkillStatus::Mismatch,
    }
}

/// One installed skill found by `--check`.
struct SkillInstall {
    agent: SkillAgent,
    scope: SkillScope,
    path: PathBuf,
    /// For project installs found above the current directory, the directory
    /// the reinstall must run from (project scope installs relative to cwd).
    run_from: Option<PathBuf>,
    version: Option<String>,
    status: SkillStatus,
}

impl SkillInstall {
    fn reinstall_command(&self) -> String {
        // Shell-neutral on purpose: the directory to run from (`run_from`) is
        // reported separately rather than chained with `cd … &&`, which
        // PowerShell 5.1 and cmd.exe don't accept.
        format!(
            "codemark install-skill --agent {} --scope {} --force",
            agent_name(self.agent),
            scope_name(self.scope)
        )
    }
}

/// Directories that may hold a project-scope install, nearest first: `cwd`
/// and its ancestors up to the enclosing repository root (the first directory
/// containing `.git`). Without a repository only `cwd` is searched, so a
/// stray install higher up the filesystem isn't misattributed to this project.
fn project_bases(cwd: &Path) -> Vec<PathBuf> {
    let mut bases = Vec::new();
    for dir in cwd.ancestors() {
        bases.push(dir.to_path_buf());
        if dir.join(".git").exists() {
            return bases;
        }
    }
    vec![cwd.to_path_buf()]
}

/// Scan the install roots (optionally narrowed to one agent/scope) for
/// installed codemark skills and classify each against `binary`.
///
/// User installs live under `home`; project installs are searched from `cwd`
/// up to the repository root so `--check` works from any subdirectory. A
/// scope whose base directory is `None` is skipped.
fn scan_installs(
    agent: Option<SkillAgent>,
    scope: Option<SkillScope>,
    binary: &str,
    home: Option<&Path>,
    cwd: Option<&Path>,
) -> Vec<SkillInstall> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut found = Vec::new();
    for a in ALL_AGENTS.into_iter().filter(|a| agent.is_none_or(|x| x == *a)) {
        for s in ALL_SCOPES.into_iter().filter(|s| scope.is_none_or(|x| x == *s)) {
            let bases = match (s, home, cwd) {
                (SkillScope::User, Some(home), _) => vec![home.to_path_buf()],
                (SkillScope::Project, _, Some(cwd)) => project_bases(cwd),
                _ => continue,
            };
            for base in bases {
                let path = base.join(skills_rel_dir(a, s)).join(SKILL_NAME);
                if !path.join("SKILL.md").is_file() {
                    continue;
                }
                // User and project roots coincide when run from $HOME; compare
                // canonical paths so symlinked prefixes (macOS /var ->
                // /private/var) don't report the same install twice.
                let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);
                let version = installed_skill_version(&path);
                let status = compare_versions(version.as_deref(), binary);
                tracing::debug!(
                    target: "codemark::skill",
                    agent = ?a,
                    scope = ?s,
                    path = %path.display(),
                    version = ?version,
                    binary = %binary,
                    status = status.as_str(),
                    "checked installed skill"
                );
                let run_from =
                    (s == SkillScope::Project && Some(base.as_path()) != cwd).then_some(base);
                found.push(SkillInstall { agent: a, scope: s, path, run_from, version, status });
            }
        }
    }
    tracing::debug!(
        target: "codemark::skill",
        agent = ?agent,
        scope = ?scope,
        home = ?home,
        cwd = ?cwd,
        installs = found.len(),
        "skill install scan complete"
    );
    found
}

/// Handle `codemark install-skill --check`.
fn handle_check(
    mode: &OutputMode,
    agent: Option<SkillAgent>,
    scope: Option<SkillScope>,
) -> Result<()> {
    // Resolve only the base directories the selected scope needs, so e.g.
    // `--scope user` still works when the working directory was deleted.
    let home = if scope != Some(SkillScope::Project) { Some(home_dir()?) } else { None };
    let cwd = if scope != Some(SkillScope::User) { Some(current_dir()?) } else { None };
    let installs = scan_installs(agent, scope, BINARY_VERSION, home.as_deref(), cwd.as_deref());
    let stale = installs.iter().filter(|i| i.status.needs_reinstall()).count();
    if stale > 0 {
        tracing::info!(target: "codemark::skill", stale, "installed skill(s) don't match the binary");
    }

    match mode {
        OutputMode::Json => {
            let items: Vec<_> = installs
                .iter()
                .map(|i| {
                    serde_json::json!({
                        "agent": agent_name(i.agent),
                        "scope": scope_name(i.scope),
                        "path": i.path.display().to_string(),
                        "skill_version": i.version,
                        "status": i.status.as_str(),
                        "reinstall_command": i.status.needs_reinstall().then(|| i.reinstall_command()),
                        "run_from": i.run_from.as_ref().map(|d| d.display().to_string()),
                    })
                })
                .collect();
            crate::cli::output::write_json_success(&serde_json::json!({
                "binary_version": BINARY_VERSION,
                "ok": stale == 0,
                "installs": items,
            }))?;
        }
        _ => print!("{}", render_check_report(&installs, BINARY_VERSION)),
    }
    Ok(())
}

/// Human-readable `--check` report.
fn render_check_report(installs: &[SkillInstall], binary: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if installs.is_empty() {
        let _ = writeln!(
            out,
            "No codemark skill installed. Install one with: codemark install-skill --agent <agent> --scope <scope>"
        );
        return out;
    }
    for i in installs {
        let label = format!("{} ({})", agent_name(i.agent), scope_name(i.scope));
        let shown = i.version.as_deref().unwrap_or("unknown");
        match i.status {
            SkillStatus::Current => {
                let _ = writeln!(out, "ok        {label}: skill {shown} matches codemark {binary}");
            }
            SkillStatus::Dev => {
                let _ = writeln!(
                    out,
                    "dev       {label}: skill {shown}, codemark {binary} (development build, not compared)"
                );
            }
            SkillStatus::Mismatch | SkillStatus::Unversioned => {
                let from = i
                    .run_from
                    .as_ref()
                    .map(|d| format!(" (from {})", d.display()))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "mismatch  {label}: your codemark skill ({shown}) doesn't match the codemark binary ({binary}). Run{from}: {}",
                    i.reinstall_command()
                );
            }
        }
        let _ = writeln!(out, "          {}", i.path.display());
    }
    out
}

fn agent_name(agent: SkillAgent) -> &'static str {
    match agent {
        SkillAgent::Claude => "claude",
        SkillAgent::Copilot => "copilot",
        SkillAgent::Agents => "agents",
        SkillAgent::Gemini => "gemini",
    }
}

fn scope_name(scope: SkillScope) -> &'static str {
    match scope {
        SkillScope::User => "user",
        SkillScope::Project => "project",
    }
}

/// Resolve the skills *root* directory (the parent that will contain the
/// `codemark/` skill folder) for a given agent and scope.
fn skill_install_root(agent: SkillAgent, scope: SkillScope) -> Result<PathBuf> {
    let base = match scope {
        SkillScope::User => home_dir()?,
        SkillScope::Project => current_dir()?,
    };
    Ok(base.join(skills_rel_dir(agent, scope)))
}

/// Path of the skills root relative to the scope's base directory.
fn skills_rel_dir(agent: SkillAgent, scope: SkillScope) -> &'static str {
    match (agent, scope) {
        (SkillAgent::Claude, _) => ".claude/skills",
        (SkillAgent::Copilot, SkillScope::User) => ".copilot/skills",
        (SkillAgent::Copilot, SkillScope::Project) => ".github/skills",
        (SkillAgent::Agents, _) => ".agents/skills",
        (SkillAgent::Gemini, _) => ".gemini/skills",
    }
}

fn home_dir() -> Result<PathBuf> {
    codemark_core::config::home_dir()
        .ok_or_else(|| Error::Operation("could not determine home directory".to_string()))
}

fn current_dir() -> Result<PathBuf> {
    std::env::current_dir()
        .map_err(|e| Error::Operation(format!("could not determine current directory: {e}")))
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
///
/// For project scope the SKILL.md reference is written as a path relative to the
/// commands file so the `.gemini/` directory stays portable when committed and
/// shared with teammates; for user scope an absolute path under the home
/// directory is fine and unambiguous.
fn write_gemini_shim(skills_root: &Path, scope: SkillScope) -> Result<()> {
    // skills_root is `<base>/.gemini/skills`; commands live in `<base>/.gemini/commands`.
    let Some(gemini_root) = skills_root.parent() else {
        return Ok(());
    };
    let commands_dir = gemini_root.join("commands");
    std::fs::create_dir_all(&commands_dir)
        .map_err(|e| Error::Operation(format!("failed to create Gemini commands dir: {e}")))?;

    // Commands live in `.gemini/commands` and the skill in `.gemini/skills/codemark`,
    // so the path from a command file to the skill is `../skills/<name>/SKILL.md`.
    let skill_ref = match scope {
        SkillScope::Project => format!("../skills/{SKILL_NAME}/SKILL.md"),
        SkillScope::User => skills_root.join(SKILL_NAME).join("SKILL.md").display().to_string(),
    };
    let toml = format!(
        "description = \"Manage structural code bookmarks with codemark\"\nprompt = \"\"\"\nFollow the instructions in the codemark skill at {skill_ref} to fulfill the user's request.\n\"\"\"\n"
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
    let agent_name = agent_name(agent);

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

    #[test]
    fn test_validate_version() {
        for ok in ["0.7.3", "1.2.3-rc.1", "v0.1.0", "2024_01", "1.0.0+build.5"] {
            assert!(validate_version(ok).is_ok(), "expected '{ok}' to be valid");
        }
        for bad in ["", "../etc", "0.7.3/foo", "1 2", "a;b", "tag?x=1"] {
            assert!(validate_version(bad).is_err(), "expected '{bad}' to be invalid");
        }
    }
    #[test]
    fn test_compare_versions() {
        assert_eq!(compare_versions(Some("0.8.0"), "0.8.0"), SkillStatus::Current);
        assert_eq!(compare_versions(Some("v0.8.0"), "0.8.0"), SkillStatus::Current);
        assert_eq!(compare_versions(Some("0.7.30"), "0.8.0"), SkillStatus::Mismatch);
        assert_eq!(compare_versions(Some("0.8.0"), "0.8.1"), SkillStatus::Mismatch);
        assert_eq!(compare_versions(None, "0.8.0"), SkillStatus::Unversioned);
        assert_eq!(compare_versions(Some("0.0.0-dev"), "0.8.0"), SkillStatus::Dev);
        assert_eq!(compare_versions(Some("0.8.0"), "0.9.0-dev"), SkillStatus::Dev);
        assert!(SkillStatus::Mismatch.needs_reinstall());
        assert!(SkillStatus::Unversioned.needs_reinstall());
        assert!(!SkillStatus::Current.needs_reinstall());
        assert!(!SkillStatus::Dev.needs_reinstall());
    }

    #[test]
    fn test_installed_version_prefers_version_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("SKILL.md"),
            "---\nname: codemark\nmetadata:\n  codemark-version: \"0.7.0\"\n---\n# body\n",
        )
        .unwrap();
        assert_eq!(installed_skill_version(dir.path()).as_deref(), Some("0.7.0"));

        std::fs::write(dir.path().join(VERSION_FILE), "0.8.0\n").unwrap();
        assert_eq!(installed_skill_version(dir.path()).as_deref(), Some("0.8.0"));
    }

    #[test]
    fn test_frontmatter_version_ignores_body() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("SKILL.md"),
            "---\nname: codemark\n---\ncodemark-version: 1.2.3\n",
        )
        .unwrap();
        assert_eq!(installed_skill_version(dir.path()), None);
    }

    #[test]
    fn test_repo_skill_carries_dev_placeholder() {
        // The release workflow rewrites this placeholder; keep both markers in sync.
        let skill = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extras/skills/codemark");
        assert_eq!(read_version_file(&skill).as_deref(), Some("0.0.0-dev"));
        assert_eq!(read_frontmatter_version(&skill).as_deref(), Some("0.0.0-dev"));
    }

    #[test]
    fn test_record_installed_version_pins_unmarked_archive() {
        // Archives published before release stamping carry no VERSION file.
        let dir = tempfile::tempdir().unwrap();
        record_installed_version(dir.path(), "0.7.30").unwrap();
        assert_eq!(read_version_file(dir.path()).as_deref(), Some("0.7.30"));
    }

    #[test]
    fn test_render_check_report() {
        let installs = vec![
            SkillInstall {
                agent: SkillAgent::Claude,
                scope: SkillScope::User,
                path: PathBuf::from("/h/.claude/skills/codemark"),
                run_from: None,
                version: Some("0.7.30".into()),
                status: SkillStatus::Mismatch,
            },
            SkillInstall {
                agent: SkillAgent::Copilot,
                scope: SkillScope::Project,
                path: PathBuf::from("/p/.github/skills/codemark"),
                run_from: None,
                version: Some("0.8.0".into()),
                status: SkillStatus::Current,
            },
        ];
        let out = render_check_report(&installs, "0.8.0");
        assert!(out.contains(
            "your codemark skill (0.7.30) doesn't match the codemark binary (0.8.0). Run: codemark install-skill --agent claude --scope user --force"
        ));
        assert!(out.contains("ok        copilot (project): skill 0.8.0 matches codemark 0.8.0"));
        assert!(render_check_report(&[], "0.8.0").starts_with("No codemark skill installed"));
    }
    fn install_skill_at(skills_root: &Path, version: Option<&str>) {
        let dir = skills_root.join(SKILL_NAME);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: codemark\n---\n").unwrap();
        if let Some(v) = version {
            std::fs::write(dir.join(VERSION_FILE), v).unwrap();
        }
    }

    #[test]
    fn test_scan_finds_project_install_above_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        let sub = repo.join("src/deep");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        install_skill_at(&repo.join(".claude/skills"), Some("0.7.30"));
        install_skill_at(&home.join(".agents/skills"), None);

        let found = scan_installs(None, None, "0.8.0", Some(&home), Some(&sub));
        assert_eq!(found.len(), 2);

        let project = found.iter().find(|i| i.scope == SkillScope::Project).unwrap();
        assert_eq!(project.agent, SkillAgent::Claude);
        assert_eq!(project.status, SkillStatus::Mismatch);
        assert_eq!(project.run_from.as_deref(), Some(repo.as_path()));
        assert_eq!(
            project.reinstall_command(),
            "codemark install-skill --agent claude --scope project --force"
        );
        assert!(
            render_check_report(&found, "0.8.0")
                .contains(&format!("Run (from {}): codemark install-skill", repo.display()))
        );

        let user = found.iter().find(|i| i.scope == SkillScope::User).unwrap();
        assert_eq!(user.status, SkillStatus::Unversioned);
        assert_eq!(
            user.reinstall_command(),
            "codemark install-skill --agent agents --scope user --force"
        );

        // Filters narrow the scan.
        assert!(
            scan_installs(Some(SkillAgent::Gemini), None, "0.8.0", Some(&home), Some(&sub))
                .is_empty()
        );
        assert_eq!(
            scan_installs(None, Some(SkillScope::User), "0.8.0", Some(&home), None).len(),
            1
        );
    }

    #[test]
    fn test_scan_stops_at_repo_root_and_dedupes_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let repo = home.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        install_skill_at(&home.join(".claude/skills"), Some("0.8.0"));

        // The install above the repo root belongs to the user scope only.
        let found = scan_installs(None, None, "0.8.0", Some(&home), Some(&repo));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].scope, SkillScope::User);

        // Run from $HOME: user and project roots coincide, reported once.
        let found = scan_installs(None, None, "0.8.0", Some(&home), Some(&home));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].status, SkillStatus::Current);
    }

    #[test]
    fn test_record_installed_version_rejects_conflicting_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(VERSION_FILE), "0.7.30\n").unwrap();
        assert!(record_installed_version(dir.path(), "0.8.0").is_err());
        assert_eq!(read_version_file(dir.path()).as_deref(), Some("0.7.30"));

        // Matching (modulo `v`) and dev-placeholder markers are accepted.
        assert!(record_installed_version(dir.path(), "v0.7.30").is_ok());
        std::fs::write(dir.path().join(VERSION_FILE), "0.0.0-dev\n").unwrap();
        record_installed_version(dir.path(), "0.8.0").unwrap();
        assert_eq!(read_version_file(dir.path()).as_deref(), Some("0.8.0"));
    }
}
