//! Resumable guided tour. Lessons operate only in a dedicated data-directory
//! workspace and remain useful when no live provider has been verified.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::promptui::{self, MenuResult};

const TOUR_SCHEMA_VERSION: u32 = 1;
const LESSON_COUNT: usize = 7;

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub restart: bool,
    pub non_interactive: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct State {
    schema_version: u32,
    next_lesson: usize,
    completed: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            schema_version: TOUR_SCHEMA_VERSION,
            next_lesson: 0,
            completed: false,
        }
    }
}

pub fn run(options: Options) -> Result<bool> {
    if !options.non_interactive
        && (!std::io::stdin().is_terminal() || !std::io::stdout().is_terminal())
    {
        anyhow::bail!("tour needs a terminal; use `aishe tour --non-interactive`");
    }
    let root = tour_root().context("data directory is unavailable")?;
    let state_path = root.join("state.json");
    let workspace = root.join("workspace");
    if options.restart {
        remove_if_exists(&state_path)?;
        if workspace.exists() {
            std::fs::remove_dir_all(&workspace)
                .with_context(|| format!("resetting tour workspace {}", workspace.display()))?;
        }
    }
    std::fs::create_dir_all(&workspace)?;
    set_private_directory(&root);
    set_private_directory(&workspace);
    let mut state = load_state(&state_path)?.unwrap_or_default();
    if state.completed && !options.restart {
        println!("aishe tour is complete (use `aishe tour --restart` to run it again)");
        return Ok(true);
    }

    promptui::header(
        "aishe guided tour",
        &format!(
            "workspace: {}",
            crate::commands::display_safe(&workspace.display().to_string())
        ),
        "Your current directory and project files are never changed.",
    );

    while state.next_lesson < LESSON_COUNT {
        let lesson = state.next_lesson;
        print_lesson(lesson, &workspace)?;
        if !options.non_interactive {
            let choices = vec![
                "Continue".into(),
                "Skip this lesson".into(),
                "Exit and resume later".into(),
            ];
            match promptui::menu(
                &format!("Lesson {} of {LESSON_COUNT}", lesson + 1),
                &choices,
                0,
                false,
                "Progress is saved after every lesson. No credentials are stored.",
            )? {
                MenuResult::Selected(0) | MenuResult::Selected(1) => {}
                _ => {
                    save_state(&state_path, &state)?;
                    println!(
                        "  Tour paused. Resume with `aishe tour` at lesson {}.",
                        lesson + 1
                    );
                    return Ok(false);
                }
            }
        }
        state.next_lesson += 1;
        save_state(&state_path, &state)?;
    }
    state.completed = true;
    save_state(&state_path, &state)?;
    println!("\n  Tour complete. Run `aishe`, then type a command or a question.");
    Ok(true)
}

fn print_lesson(index: usize, workspace: &Path) -> Result<()> {
    match index {
        0 => {
            println!("\n  1. Normal shell commands");
            println!(
                "     `ls -la`, pipes, aliases, completion, and plugins run in your real Zsh."
            );
            println!("     Prefix with `!` if a command name is ambiguous.");
        }
        1 => {
            println!("\n  2. Natural-language routing");
            println!(
                "     Type `what is using the most disk space` or prefix any request with `?`."
            );
            println!("     Aishe changes the input color when it recognizes the AI route.");
            let verified = Config::load_quiet()
                .ok()
                .flatten()
                .and_then(|config| crate::capabilities::load(&config))
                .is_some_and(|report| report.live_verified());
            println!(
                "     Live provider: {}.",
                if verified {
                    "previously verified and ready"
                } else {
                    "not verified; lesson stays offline (run `aishe doctor --live`)"
                }
            );
        }
        2 => {
            println!("\n  3. Suggest mode");
            println!(
                "     Suggestions are placed on the command line for review; edit or cancel them."
            );
            println!("     Alt-Enter forces the current buffer through Aishe.");
        }
        3 => {
            println!("\n  4. Recover from failures");
            println!("     After a failed command, press Ctrl-X Ctrl-F to request a reviewed fix.");
            println!("     Aishe never reruns a state-changing command merely to diagnose it.");
        }
        4 => {
            println!("\n  5. File change and undo");
            prove_undo(workspace)?;
            println!("     Created a tour-only file, journaled it, and proved undo removed it.");
        }
        5 => {
            println!("\n  6. Modes and safety");
            println!("     Shift-Tab cycles suggest → auto → yolo for this session.");
            println!(
                "     `aishe profile` shows the persistent profile; `aishe readiness` checks yolo."
            );
        }
        6 => {
            let paths = crate::diagnostics::resolved_paths();
            println!("\n  7. Your persistent state");
            println!(
                "     config:  {}",
                crate::commands::display_safe(&paths.config.display().to_string())
            );
            println!(
                "     history: {}",
                crate::commands::display_safe(&paths.history.display().to_string())
            );
            println!(
                "     data:    {}",
                crate::commands::display_safe(&paths.data_dir.display().to_string())
            );
            println!(
                "     tasks:   {}",
                crate::commands::display_safe(&paths.data_dir.join("tasks").display().to_string())
            );
            println!("     Updates replace the binary, not these files.");
        }
        _ => {}
    }
    Ok(())
}

fn prove_undo(workspace: &Path) -> Result<()> {
    let file = workspace.join("undo-demo.txt");
    let journal = workspace.join("undo-demo.jsonl");
    remove_if_exists(&file)?;
    remove_if_exists(&journal)?;
    let previous = std::env::var_os("AISHE_UNDO_JOURNAL");
    std::env::set_var("AISHE_UNDO_JOURNAL", &journal);
    crate::undo::record(&file, false, None, "tour", "guided tour undo demo");
    std::fs::write(&file, b"tour-only\n")?;
    let result = crate::undo::undo_last();
    match previous {
        Some(value) => std::env::set_var("AISHE_UNDO_JOURNAL", value),
        None => std::env::remove_var("AISHE_UNDO_JOURNAL"),
    }
    let undone = result?.context("tour undo journal did not contain the demo change")?;
    if file.exists() || !undone.errors.is_empty() {
        anyhow::bail!("tour undo proof failed: {}", undone.errors.join("; "));
    }
    Ok(())
}

fn tour_root() -> Option<PathBuf> {
    crate::config::data_root().map(|root| root.join("aishe").join("tour"))
}

fn load_state(path: &Path) -> Result<Option<State>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let state: State = serde_json::from_slice(&bytes)?;
    if state.schema_version != TOUR_SCHEMA_VERSION {
        anyhow::bail!("tour state is from a newer schema; use `aishe tour --restart`");
    }
    Ok(Some(state))
}

fn save_state(path: &Path, state: &State) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        set_private_directory(parent);
    }
    crate::config::write_atomic(path, &serde_json::to_vec_pretty(state)?)?;
    set_private(path);
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn set_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private(_path: &Path) {}

#[cfg(unix)]
fn set_private_directory(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_roundtrip_preserves_resume_index() {
        let path = std::env::temp_dir().join(format!(
            "aishe-tour-state-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let state = State {
            schema_version: TOUR_SCHEMA_VERSION,
            next_lesson: 4,
            completed: false,
        };
        save_state(&path, &state).unwrap();
        let loaded = load_state(&path).unwrap().unwrap();
        assert_eq!(loaded.next_lesson, 4);
        std::fs::remove_file(path).ok();
    }
}
