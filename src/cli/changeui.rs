//! Review exact task changes alongside observed checks before selecting effects.

use std::collections::BTreeSet;
use std::io::IsTerminal;

use anyhow::Result;

use crate::background::{self, ChangeFile, ChangeReview};
use crate::promptui::{self, PickerResult};

pub fn review_command(id: &str, json: bool) -> Result<u8> {
    let review = background::task_change_review(id)?;
    if json {
        crate::cli::json_contract::print_object(&review)?;
    } else {
        for line in review_lines(&review) {
            println!("{line}");
        }
    }
    Ok(0)
}

pub fn apply_command(id: &str, revision: &str, files: &[usize], hunks: &[usize]) -> Result<u8> {
    let applied = background::apply_task_changes(id, revision, files, hunks)?;
    println!(
        "Applied {} hunk(s); {} remaining{}.",
        applied.applied_hunks.len(),
        applied.remaining_hunks,
        if applied.complete { " · complete" } else { "" }
    );
    Ok(0)
}

pub fn review_lines(review: &ChangeReview) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{} files · {} remaining hunks · {} already applied",
            review.files.len(),
            review.remaining_hunks,
            review.applied_hunks
        ),
        review.check_summary.label(),
        safe(&review.evidence_caveat),
    ];
    for issue in review
        .unresolved
        .iter()
        .chain(&review.check_summary.unresolved)
        .take(16)
    {
        lines.push(format!("Needs attention: {}", safe(issue)));
    }
    for check in review.checks.iter().rev().take(8) {
        lines.push(check_label(check, review.evidence_workspace_revision));
    }
    lines.push(format!("Revision: {}", review.revision));
    for file in &review.files {
        lines.push(format!(
            "File {} · {} · {:?}{}",
            file.id,
            file_label(file),
            file.kind,
            if file.applied { " · applied" } else { "" }
        ));
        if let Some(reason) = &file.limitation {
            lines.push(format!("  {}", safe(reason)));
        }
        for hunk in &file.hunks {
            lines.push(format!(
                "  Hunk {}{} · {}",
                hunk.id,
                if hunk.applied { " · applied" } else { "" },
                safe(&hunk.header)
            ));
            lines.extend(hunk.lines.iter().map(|line| safe(line)));
            if hunk.clipped {
                lines.push(
                    "  Display clipped; whole selection still uses the exact recorded patch."
                        .into(),
                );
            }
        }
    }
    lines
}

pub fn review_and_apply(id: &str) -> Result<()> {
    let review = background::task_change_review(id)?;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        for line in review_lines(&review) {
            println!("{line}");
        }
        return Ok(());
    }
    promptui::section("Review task changes");
    println!("{}", review.check_summary.label());
    println!("{}", safe(&review.evidence_caveat));
    for check in review.checks.iter().rev().take(4) {
        println!("{}", check_label(check, review.evidence_workspace_revision));
    }
    for issue in review
        .unresolved
        .iter()
        .chain(&review.check_summary.unresolved)
        .take(8)
    {
        promptui::warning(&safe(issue));
    }
    if !review.can_apply || review.remaining_hunks == 0 {
        for line in review_lines(&review).into_iter().take(64) {
            println!("{line}");
        }
        return Ok(());
    }
    let mut selected = BTreeSet::new();
    loop {
        let files = review
            .files
            .iter()
            .filter(|file| !file.applied)
            .collect::<Vec<_>>();
        let mut choices = files
            .iter()
            .map(|file| {
                let remaining = file.hunks.iter().filter(|h| !h.applied).collect::<Vec<_>>();
                let picked = remaining
                    .iter()
                    .filter(|h| selected.contains(&h.id))
                    .count();
                format!(
                    "[{}] File {} · {} · {picked}/{} selected{}",
                    if picked == remaining.len() && picked > 0 {
                        "x"
                    } else if picked > 0 {
                        "-"
                    } else {
                        " "
                    },
                    file.id,
                    file_label(file),
                    remaining.len(),
                    if file.selectable {
                        ""
                    } else {
                        " · unavailable"
                    }
                )
            })
            .collect::<Vec<_>>();
        let apply_index = choices.len();
        choices.push(format!("Apply selected changes ({} hunks)", selected.len()));
        choices.push("Back to task".into());
        let PickerResult::Use(index) =
            promptui::filter_picker("Review task changes", &choices, choices.len() - 1)?
        else {
            return Ok(());
        };
        if let Some(file) = files.get(index) {
            select_file(file, &mut selected)?;
        } else if index == apply_index {
            if selected.is_empty() {
                promptui::warning("Select a file or hunk first.");
                continue;
            }
            println!("{}", review.check_summary.label());
            println!("{}", safe(&review.evidence_caveat));
            if promptui::confirm(
                &format!(
                    "Apply {} selected hunks to the source repository?",
                    selected.len()
                ),
                false,
            )? != Some(true)
            {
                continue;
            }
            // Every selected atomic file contains all of its exact hunks. The
            // engine revalidates the revision and source preimages under lock.
            let whole_files = review
                .files
                .iter()
                .filter(|file| {
                    !file.hunks.is_empty()
                        && file
                            .hunks
                            .iter()
                            .all(|h| !h.applied && selected.contains(&h.id))
                })
                .map(|file| file.id)
                .collect::<Vec<_>>();
            let individual = selected
                .iter()
                .copied()
                .filter(|id| {
                    !review
                        .files
                        .iter()
                        .filter(|file| whole_files.contains(&file.id))
                        .any(|file| file.hunks.iter().any(|h| h.id == *id))
                })
                .collect::<Vec<_>>();
            apply_command(id, &review.revision, &whole_files, &individual)?;
            return Ok(());
        } else {
            return Ok(());
        }
    }
}

fn select_file(file: &ChangeFile, selected: &mut BTreeSet<usize>) -> Result<()> {
    println!("File {} · {} · {:?}", file.id, file_label(file), file.kind);
    if let Some(reason) = &file.limitation {
        println!("{}", safe(reason));
    }
    let mut preview_left = 120;
    for hunk in &file.hunks {
        println!(
            "Hunk {}{} · {}",
            hunk.id,
            if hunk.applied { " · applied" } else { "" },
            safe(&hunk.header)
        );
        for line in hunk.lines.iter().take(preview_left) {
            println!("{}", safe(line));
        }
        preview_left = preview_left.saturating_sub(hunk.lines.len());
    }
    if file.hunks.iter().map(|h| h.lines.len()).sum::<usize>() > 120 {
        println!("Display clipped; use `aishe task review --json` with this task ID for the bounded detailed preview.");
    }
    loop {
        let remaining = file.hunks.iter().filter(|h| !h.applied).collect::<Vec<_>>();
        let mut labels = remaining
            .iter()
            .map(|h| {
                format!(
                    "[{}] Hunk {} · {}{}",
                    if selected.contains(&h.id) { "x" } else { " " },
                    h.id,
                    safe(&h.header),
                    if h.selectable {
                        ""
                    } else {
                        " · whole file only"
                    }
                )
            })
            .collect::<Vec<_>>();
        let all = labels.len();
        labels.push("Toggle whole file".into());
        labels.push("Back to files".into());
        let PickerResult::Use(index) =
            promptui::filter_picker("File changes", &labels, labels.len() - 1)?
        else {
            return Ok(());
        };
        if let Some(hunk) = remaining.get(index) {
            if !hunk.selectable {
                promptui::warning("Choose Toggle whole file for this atomic change.");
                continue;
            }
            if !selected.remove(&hunk.id) {
                selected.insert(hunk.id);
            }
        } else if index == all {
            if !file.selectable {
                promptui::warning("This file cannot be selected safely.");
                continue;
            }
            let remove = remaining.iter().all(|h| selected.contains(&h.id));
            for hunk in remaining {
                if remove {
                    selected.remove(&hunk.id);
                } else {
                    selected.insert(hunk.id);
                }
            }
        } else {
            return Ok(());
        }
    }
}

fn safe(value: &str) -> String {
    crate::commands::display_safe(&crate::redact::redact(value))
}

fn check_label(check: &crate::tasks::CheckEvidence, workspace_revision: u64) -> String {
    let status = if check.stale(workspace_revision) {
        format!("Stale ({:?})", check.outcome)
    } else {
        format!("{:?}", check.outcome)
    };
    format!(
        "  {status} · exit {} · {}",
        check
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "not recorded".into()),
        safe(&check.command)
    )
}

fn file_label(file: &ChangeFile) -> String {
    match &file.old_path {
        Some(old) => format!("{} → {}", safe(old), safe(&file.path)),
        None => safe(&file.path),
    }
}
