//! `tour copy` / `tour move` (and their `collection` mirrors): clone a
//! collection as a template, or move it to another branch. Both fork every
//! step; see `codemark_core::engine::transfer` and ADR 0001 (discussion #300).

use serde::Serialize;

use crate::cli::output::{OutputMode, write_json, write_json_success};
use crate::cli::*;
use codemark_core::engine::transfer::{
    self as engine, OnUnmatched, StepStatus, TransferMode, TransferOptions, TransferOutcome,
    TransferPlan, TransferTarget,
};
use codemark_core::error::{Error, Result};
use codemark_core::git::context as git_context;

use super::{generate_embedding_for_bookmark, load_config, open_db_for_write};

/// JSON report written for every copy/move, applied or not.
#[derive(Serialize)]
struct TransferReport<'a> {
    applied: bool,
    dry_run: bool,
    on_unmatched: OnUnmatched,
    plan: &'a TransferPlan,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<&'a TransferOutcome>,
}

pub async fn handle_collection_copy(
    cli: &Cli,
    mode: &OutputMode,
    args: &CollectionCopyArgs,
) -> Result<()> {
    run(cli, mode, &args.name, Some(args.new_name.as_deref()), &args.transfer).await
}

pub async fn handle_collection_move(
    cli: &Cli,
    mode: &OutputMode,
    args: &CollectionMoveArgs,
) -> Result<()> {
    run(cli, mode, &args.name, None, &args.transfer).await
}

/// `copy_as` is `Some(..)` for a copy (holding the optional `--as` name) and
/// `None` for a move.
async fn run(
    cli: &Cli,
    mode: &OutputMode,
    name: &str,
    copy_as: Option<Option<&str>>,
    args: &TransferArgs,
) -> Result<()> {
    let on_unmatched: OnUnmatched = args.on_unmatched.parse()?;
    let db = open_db_for_write(cli)?;
    let collection = db
        .get_collection_by_name(name)?
        .ok_or_else(|| Error::Input(format!("collection '{name}' not found")))?;

    let repo_root = engine::repo_root_for(&db);
    let ctx = git_context::detect_context(&repo_root);
    let (target_branch, target) = match &args.to {
        Some(branch) => {
            let sha = engine::resolve_revision(&repo_root, branch)?;
            (branch.clone(), TransferTarget::Revision { rev: branch.clone(), sha })
        }
        None => {
            let branch = ctx.as_ref().and_then(|c| c.branch_name.clone()).ok_or_else(|| {
                Error::Input("not on a branch (detached HEAD?); pass --to <branch>".into())
            })?;
            let head = ctx.as_ref().and_then(|c| c.head_commit.clone());
            (branch, TransferTarget::WorkingTree { head })
        }
    };

    let transfer_mode = match copy_as {
        Some(new_name) => TransferMode::Copy {
            new_name: new_name
                .map(str::to_string)
                .unwrap_or_else(|| default_copy_name(name, &target_branch)),
        },
        None => TransferMode::Move,
    };
    let options = TransferOptions { target_branch, target, allow_fuzzy: args.allow_fuzzy };
    let plan = engine::plan_transfer(&db, &collection, &options).await?;

    if args.dry_run {
        return report(mode, &plan, None, on_unmatched, true);
    }
    if on_unmatched == OnUnmatched::Fail && plan.has_unmatched() {
        // Show which steps failed (and the prompt to rebuild them from), then
        // exit non-zero without writing anything.
        report(mode, &plan, None, on_unmatched, false)?;
        let n = plan.unmatched().count();
        eprintln!(
            "codemark: {n} step(s) don't match on '{}'; nothing was written. \
             Re-run with --on-unmatched drop to leave them out{}.",
            plan.target_branch,
            if args.allow_fuzzy { "" } else { ", or --allow-fuzzy to accept looser matches" }
        );
        std::process::exit(Error::Input(String::new()).exit_code());
    }

    let outcome = engine::apply_transfer(
        &db,
        &plan,
        &transfer_mode,
        on_unmatched,
        args.created_by.as_deref(),
    )?;

    // Forks are new rows, so they need their own embeddings for semantic
    // search. Failures here never undo the transfer.
    let config = load_config(cli);
    for f in &outcome.forked {
        if let Ok(Some(bm)) = db.get_bookmark(&f.fork_id) {
            let _ = generate_embedding_for_bookmark(cli, &config, &bm).await;
        }
    }

    report(mode, &plan, Some(&outcome), on_unmatched, false)
}

/// `<name>-<branch>`, with the branch reduced to the characters collection
/// names allow (lowercase alphanumerics and hyphens).
fn default_copy_name(name: &str, branch: &str) -> String {
    let slug: String = branch
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    format!("{name}-{slug}")
}

fn report(
    mode: &OutputMode,
    plan: &TransferPlan,
    outcome: Option<&TransferOutcome>,
    on_unmatched: OnUnmatched,
    dry_run: bool,
) -> Result<()> {
    let applied = outcome.is_some();
    match mode {
        OutputMode::Json => {
            let report = TransferReport { applied, dry_run, on_unmatched, plan, outcome };
            if applied || dry_run {
                write_json_success(&report)?;
            } else {
                write_json(&serde_json::json!({
                    "success": false,
                    "error": format!(
                        "{} step(s) don't match on '{}'",
                        plan.unmatched().count(),
                        plan.target_branch
                    ),
                    "data": report,
                }))?;
            }
        }
        _ => {
            let head = match outcome {
                Some(o) if o.mode == "copy" => {
                    format!(
                        "Copied '{}' to '{}' on {}",
                        plan.collection, o.collection, plan.target_branch
                    )
                }
                Some(_) => format!("Moved '{}' to {}", plan.collection, plan.target_branch),
                None if dry_run => {
                    format!("Dry run: '{}' on {}", plan.collection, plan.target_branch)
                }
                None => format!("Not applied: '{}' on {}", plan.collection, plan.target_branch),
            };
            let commit = plan
                .target_commit
                .as_deref()
                .map(|c| &c[..c.len().min(8)])
                .unwrap_or("working tree");
            println!(
                "{head} ({commit}): {} matched, {} unmatched",
                plan.matched().count(),
                plan.unmatched().count()
            );
            for s in &plan.steps {
                let mark = if s.status == StepStatus::Matched { "✓" } else { "✗" };
                let path = if s.renamed {
                    format!("{} (renamed from {})", s.target_file_path, s.file_path)
                } else {
                    s.target_file_path.clone()
                };
                let range = s.line_range.as_deref().map(|r| format!(":{r}")).unwrap_or_default();
                let mut detail = s.method.to_string();
                if s.hash_changed && s.status == StepStatus::Matched {
                    detail.push_str(", code changed");
                }
                if let Some(reason) = &s.reason {
                    detail = reason.clone();
                }
                let note = s.notes.first().map(|n| format!(" — {n}")).unwrap_or_default();
                println!("  {mark} {}. {path}{range} [{detail}]{note}", s.position + 1);
            }
            if let Some(o) = outcome {
                if !o.dropped.is_empty() {
                    println!(
                        "  Dropped {} step(s); their bookmarks were kept: {}",
                        o.dropped.len(),
                        o.dropped.join(", ")
                    );
                }
                if !o.kept_originals.is_empty() {
                    println!(
                        "  Kept {} original(s) still used by other collections",
                        o.kept_originals.len()
                    );
                }
            } else if plan.has_unmatched()
                && on_unmatched == OnUnmatched::Fail
                && plan.prompt.is_some()
            {
                println!(
                    "  Prompt (to rebuild unmatched steps): {}",
                    plan.prompt.as_deref().unwrap_or("")
                );
            }
        }
    }
    Ok(())
}
