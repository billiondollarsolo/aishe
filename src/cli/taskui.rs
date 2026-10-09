//! A small live task drawer. Observation is read-only; controls leave raw mode
//! and require a separate, explicit choice before using the task engine.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::background::{self, Action, Record, State, TaskDetails, TaskEntry};
use crate::config::Config;
use crate::promptui::{self, PickerInput, PickerKey, RawGuard};
use crate::ui::{Motion, TerminalCapabilities};

const MAX_FRAME_ROWS: usize = 18;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Page {
    List,
    Details,
    Activity,
    Changes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Control {
    Stop,
    Resume,
    Rework,
    Apply,
    Discard,
}

#[derive(Default)]
struct Browser {
    entries: Vec<TaskEntry>,
    query: String,
    selected_id: Option<String>,
    all: bool,
    details: Option<TaskDetails>,
    patch: Vec<String>,
    scroll: usize,
    notice: Option<String>,
    show_actions: bool,
}

/// Non-terminal callers receive a bounded readable snapshot rather than an
/// interactive prompt. No provider, MCP connection or worker is initialized.
pub fn browse(config: &Config, id: Option<&str>, all: bool) -> Result<u8> {
    let project = std::env::current_dir().context("reading task browser project")?;
    let seen = std::env::var_os("AISHE_BACKGROUND_SEEN_FILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let mut browser = Browser {
        all,
        ..Browser::default()
    };
    browser.refresh(&project, true)?;
    // A global badge must never open an apparently empty list just because the
    // work belongs to another project. Keep the scope visible in the drawer.
    if !all && browser.entries.is_empty() {
        let elsewhere = background::cached_task_entries(None)?;
        if !elsewhere.is_empty() {
            browser.all = true;
            browser.entries = elsewhere;
        }
    }
    let mut page = Page::List;
    if let Some(id) = id {
        browser.open(id, seen.as_deref())?;
        page = Page::Details;
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        print_snapshot(&browser, page);
        return Ok(0);
    }
    if TerminalCapabilities::detect_stdout().motion == Motion::Static {
        return browse_static(config, &project, seen.as_deref(), browser, page);
    }
    loop {
        match interact(&project, seen.as_deref(), &mut browser, &mut page)? {
            Some(control) => {
                let Some(details) = &browser.details else {
                    continue;
                };
                let task_id = details.record.id.clone();
                // The interactive frame and raw guard have both been dropped.
                // Reading alone can never spawn, signal, apply or discard work.
                if let Err(error) = run_control(config, &details.record, control) {
                    promptui::error(&safe(&error.to_string()));
                    browser.notice = Some(safe(&error.to_string()));
                }
                browser.refresh(&project, true)?;
                browser.open(&task_id, seen.as_deref())?;
                page = Page::Details;
            }
            None => return Ok(0),
        }
    }
}

impl Browser {
    fn refresh(&mut self, project: &Path, reconcile: bool) -> Result<()> {
        let project = (!self.all).then_some(project);
        self.entries = if reconcile {
            background::task_entries(project, false)?
        } else {
            background::cached_task_entries(project)?
        };
        let matches = self.matches();
        if !matches
            .iter()
            .any(|entry| Some(&entry.id) == self.selected_id.as_ref())
        {
            self.selected_id = matches.first().map(|entry| entry.id.clone());
        }
        Ok(())
    }

    fn matches(&self) -> Vec<&TaskEntry> {
        let words = self.query.to_lowercase();
        self.entries
            .iter()
            .filter(|entry| {
                let text = format!(
                    "{} {} {} {}",
                    entry.id,
                    entry.objective,
                    state_label(entry.state),
                    entry.project.display()
                )
                .to_lowercase();
                words.split_whitespace().all(|word| text.contains(word))
            })
            .collect()
    }

    fn move_selection(&mut self, delta: isize) {
        let entries = self.matches();
        if entries.is_empty() {
            self.selected_id = None;
            return;
        }
        let selected = entries
            .iter()
            .position(|entry| Some(&entry.id) == self.selected_id.as_ref())
            .unwrap_or(0);
        let next =
            (selected as isize + delta).clamp(0, entries.len().saturating_sub(1) as isize) as usize;
        self.selected_id = Some(entries[next].id.clone());
    }

    fn open(&mut self, id: &str, seen: Option<&Path>) -> Result<()> {
        let details = background::task_details(id)?;
        if !matches!(details.record.state, State::Starting | State::Running) {
            if let Some(path) = seen {
                background::acknowledge_task(&TaskEntry::from_record(&details.record), path)?;
            }
        }
        self.selected_id = Some(id.into());
        self.details = Some(details);
        self.scroll = 0;
        self.patch.clear();
        Ok(())
    }
}

/// Own the frame independently from the raw guard so errors and EOF clear it
/// before restoring cooked terminal input.
struct Frame {
    capabilities: TerminalCapabilities,
    rows: usize,
    last: Vec<String>,
    size: (usize, usize),
}

impl Frame {
    fn draw(&mut self, lines: Vec<String>, size: (usize, usize)) {
        if self.last != lines || self.size != size {
            promptui::draw_raw_frame(&lines, &mut self.rows, &self.capabilities);
            std::io::stdout().flush().ok();
            self.last = lines;
            self.size = size;
        }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        promptui::draw_raw_frame(&[], &mut self.rows, &self.capabilities);
        std::io::stdout().flush().ok();
    }
}

fn interact(
    project: &Path,
    seen: Option<&Path>,
    browser: &mut Browser,
    page: &mut Page,
) -> Result<Option<Control>> {
    let mut input = PickerInput::open()?;
    let _raw = RawGuard::enter()?;
    let mut frame = Frame {
        capabilities: TerminalCapabilities::detect_stdout(),
        rows: 0,
        last: Vec::new(),
        size: (0, 0),
    };
    let mut last_refresh = Instant::now();
    let mut redraw = true;
    loop {
        if last_refresh.elapsed() >= Duration::from_secs(2) {
            background::refresh_task_cache()?;
            browser.refresh(project, false)?;
            if *page != Page::List {
                if let Some(id) = browser
                    .details
                    .as_ref()
                    .map(|details| details.record.id.clone())
                {
                    if let Ok(details) = background::task_details(&id) {
                        browser.details = Some(details);
                    }
                }
            }
            last_refresh = Instant::now();
            redraw = true;
        }
        let size = promptui::terminal_size();
        if redraw || frame.size != size {
            frame.draw(frame_lines(browser, *page, size), size);
            redraw = false;
        }
        let Some(key) = input.read_live_key(150)? else {
            continue;
        };
        redraw = true;
        match key {
            PickerKey::Interrupt => return Ok(None),
            PickerKey::Cancel if *page == Page::List => return Ok(None),
            PickerKey::Cancel => {
                *page = if matches!(*page, Page::Activity | Page::Changes) {
                    Page::Details
                } else {
                    Page::List
                };
                browser.scroll = 0;
                browser.notice = None;
            }
            PickerKey::Character('\x12') => {
                let scroll = browser.scroll;
                browser.refresh(project, true)?;
                if *page != Page::List {
                    if let Some(id) = browser
                        .details
                        .as_ref()
                        .map(|details| details.record.id.clone())
                    {
                        browser.open(&id, seen)?;
                        if *page == Page::Changes {
                            browser.patch = background::task_patch_lines(&id, 2_000)
                                .unwrap_or_else(|error| vec![safe(&error.to_string())]);
                        }
                        browser.scroll = scroll;
                    }
                }
                last_refresh = Instant::now();
            }
            PickerKey::Character('\t') if *page == Page::List => {
                browser.all = !browser.all;
                browser.refresh(project, true)?;
            }
            PickerKey::Enter if *page == Page::List => {
                if let Some(id) = browser.selected_id.clone() {
                    browser.open(&id, seen)?;
                    *page = Page::Details;
                }
            }
            PickerKey::Up
            | PickerKey::Down
            | PickerKey::PageUp
            | PickerKey::PageDown
            | PickerKey::Home
            | PickerKey::End => {
                let amount = match key {
                    PickerKey::Up => -1,
                    PickerKey::Down => 1,
                    PickerKey::PageUp => -8,
                    PickerKey::PageDown => 8,
                    PickerKey::Home => isize::MIN,
                    PickerKey::End => isize::MAX,
                    _ => 0,
                };
                if *page == Page::List {
                    if amount == isize::MIN {
                        browser.selected_id =
                            browser.matches().first().map(|entry| entry.id.clone());
                    } else if amount == isize::MAX {
                        browser.selected_id =
                            browser.matches().last().map(|entry| entry.id.clone());
                    } else {
                        browser.move_selection(amount);
                    }
                } else {
                    let length = body_rows(browser, *page, size.0.saturating_sub(1).max(1)).len();
                    let maximum = length.saturating_sub(body_viewport(browser, *page, size));
                    browser.scroll = if amount == isize::MIN {
                        0
                    } else if amount == isize::MAX {
                        maximum
                    } else {
                        browser.scroll.saturating_add_signed(amount).min(maximum)
                    };
                }
            }
            PickerKey::Backspace if *page == Page::List => {
                browser.query.pop();
                browser.refresh(project, false)?;
            }
            PickerKey::Character(c) if *page == Page::List && !c.is_control() => {
                if browser.query.len() < 512 {
                    browser.query.push(c);
                    browser.refresh(project, false)?;
                }
            }
            PickerKey::Character('l') if *page == Page::Details => {
                *page = Page::Activity;
                browser.scroll = 0;
            }
            PickerKey::Character('?') if *page == Page::Details => {
                browser.show_actions = !browser.show_actions;
            }
            PickerKey::Character('p') if *page == Page::Details => {
                if let Some(details) = &browser.details {
                    browser.patch = background::task_patch_lines(&details.record.id, 2_000)
                        .unwrap_or_else(|error| vec![safe(&error.to_string())]);
                    *page = Page::Changes;
                    browser.scroll = 0;
                }
            }
            PickerKey::Character(c) if *page == Page::Details => {
                if let Some(details) = &browser.details {
                    if let Some(control) = control_key(&details.record, c) {
                        return Ok(Some(control));
                    }
                }
            }
            _ => {}
        }
    }
}

fn frame_lines(browser: &Browser, page: Page, size: (usize, usize)) -> Vec<String> {
    let budget = size.1.saturating_sub(1).clamp(1, MAX_FRAME_ROWS);
    let width = size.0.saturating_sub(1).max(1);
    let title = match page {
        Page::List => "Background tasks",
        Page::Details => "Task details",
        Page::Activity => "Task activity",
        Page::Changes => "Task changes",
    };
    let mut lines = vec![format!("  {title}")];
    if budget == 1 {
        return fit_lines(lines, width, budget);
    }
    let footer = if page == Page::List {
        "Enter details · Tab project/all · Ctrl-R refresh · Esc close".to_string()
    } else if page == Page::Details {
        if browser
            .details
            .as_ref()
            .is_some_and(|details| details.record.worktree.is_some())
        {
            "l activity · p changes · ? actions · Esc back".into()
        } else {
            "l activity · ? actions · Esc back".into()
        }
    } else {
        "↑/↓ scroll · Ctrl-R refresh · Esc back".into()
    };
    if page == Page::List {
        let entries = browser.matches();
        lines.push(format!(
            "  {} · {} task{}",
            if browser.all {
                "all projects"
            } else {
                "this project"
            },
            entries.len(),
            if entries.len() == 1 { "" } else { "s" }
        ));
        if budget > 4 {
            lines.push(format!("  search: {}", browser.query));
        }
        let available = budget.saturating_sub(lines.len() + 1).max(1);
        if entries.is_empty() {
            lines.push(if browser.query.is_empty() {
                "  No background tasks. Start with aishe agent --background <goal>.".into()
            } else {
                "  No matching tasks. Backspace edits the search.".into()
            });
        } else {
            let selected = entries
                .iter()
                .position(|entry| Some(&entry.id) == browser.selected_id.as_ref())
                .unwrap_or(0);
            let start = selected
                .saturating_sub(available / 2)
                .min(entries.len().saturating_sub(available));
            for entry in entries.iter().skip(start).take(available) {
                lines.push(format!(
                    "  {} {} · {} · {}",
                    if Some(&entry.id) == browser.selected_id.as_ref() {
                        ">"
                    } else {
                        " "
                    },
                    state_label(entry.state),
                    elapsed_label(entry.elapsed_ms),
                    entry.objective
                ));
            }
            if budget > 7 && browser.all {
                if let Some(entry) = entries.get(selected) {
                    let project_line = format!("  project: {}", entry.project.display());
                    if lines.len() + 1 >= budget {
                        lines.pop();
                    }
                    lines.push(project_line);
                }
            }
        }
    } else {
        let body = body_rows(browser, page, width);
        let actions = if page == Page::Details && browser.show_actions {
            browser.details.as_ref().map(|details| {
                controls(&details.record)
                    .into_iter()
                    .map(|(key, label, _)| format!("{key} {label}"))
                    .collect::<Vec<_>>()
                    .join(" · ")
            })
        } else {
            None
        };
        let available = body_viewport(browser, page, size);
        let offset = browser.scroll.min(body.len().saturating_sub(available));
        lines.extend(
            body.into_iter()
                .skip(offset)
                .take(available)
                .map(|line| format!("  {line}")),
        );
        if let Some(actions) = actions {
            lines.push(format!("  actions: {actions}"));
        }
    }
    if budget > 2 {
        lines.push(format!("  keys: {footer}"));
    }
    fit_lines(lines, width, budget)
}

fn fit_lines(lines: Vec<String>, width: usize, budget: usize) -> Vec<String> {
    lines
        .into_iter()
        .take(budget)
        .map(|line| crate::ui::truncate_cells(&view_text(&line), width))
        .collect()
}

fn page_body(browser: &Browser, page: Page) -> Vec<String> {
    let Some(details) = &browser.details else {
        return vec!["Task unavailable. Ctrl-R refreshes the list.".into()];
    };
    match page {
        Page::Activity => {
            if details.log_lines.is_empty() {
                vec!["No activity recorded yet.".into()]
            } else {
                details.log_lines.clone()
            }
        }
        Page::Changes => {
            if browser.patch.is_empty() {
                vec!["No file changes.".into()]
            } else {
                browser.patch.clone()
            }
        }
        _ => detail_lines(details, browser.notice.as_deref()),
    }
}

fn body_rows(browser: &Browser, page: Page, width: usize) -> Vec<String> {
    page_body(browser, page)
        .into_iter()
        .flat_map(|line| crate::ui::wrap_cells(&view_text(&line), width.saturating_sub(2).max(1)))
        .collect()
}

fn body_viewport(browser: &Browser, page: Page, size: (usize, usize)) -> usize {
    size.1
        .saturating_sub(1)
        .clamp(1, MAX_FRAME_ROWS)
        .saturating_sub(2 + usize::from(page == Page::Details && browser.show_actions))
        .max(1)
}

fn detail_lines(details: &TaskDetails, notice: Option<&str>) -> Vec<String> {
    let record = &details.record;
    let mut lines = vec![
        record.objective.clone(),
        format!(
            "{} · {} · {}",
            state_label(record.state),
            elapsed_label(details.elapsed_ms()),
            record.id
        ),
        format!("Last activity: {}", details.activity),
    ];
    if let Some(checkpoint) = &details.checkpoint {
        let execution = checkpoint.execution;
        lines.push(format!(
            "Used: {}/{} turns · {}/{} tools · {}/{} network",
            execution.provider_turns,
            record.budget.max_provider_turns,
            execution.tool_calls,
            record.budget.max_tool_calls,
            execution.network_calls,
            record.budget.max_network_calls
        ));
        lines.push(format!(
            "Usage: {} input · {} output tokens · recorded cost ${:.4}",
            checkpoint.usage.input, checkpoint.usage.output, execution.cost_usd
        ));
        if let Some(result) = &checkpoint.latest_result {
            lines.push("Latest response:".into());
            lines.extend(result.lines().take(150).map(ToOwned::to_owned));
        }
        if let Some(error) = &checkpoint.last_error {
            lines.push(format!("Checkpoint: {error}"));
        }
    }
    if let Some(error) = &record.error {
        lines.push(format!("Attention: {error}"));
    }
    if let Some(notice) = notice {
        lines.push(format!("Notice: {notice}"));
    }
    lines.extend([
        format!("Model: {} / {}", record.provider, record.model),
        format!(
            "Scope: {} · network {} · {}",
            record.scope,
            record.network,
            if record.worktree.is_some() {
                "isolated worktree"
            } else {
                "source directory"
            }
        ),
        format!("Source: {}", record.source_cwd.display()),
        format!(
            "Time limit: {}m · turns {} · tools {} · network {}",
            record.budget.max_minutes,
            record.budget.max_provider_turns,
            record.budget.max_tool_calls,
            record.budget.max_network_calls
        ),
    ]);
    if record.budget.max_cost_usd > 0.0 {
        lines.push(format!("Cost limit: ${:.2}", record.budget.max_cost_usd));
    }
    if record.state == State::Completed {
        lines.push("Finished by the agent; review its result and checks before applying.".into());
    }
    if !record.plan.is_empty() {
        lines.push("Plan:".into());
        for step in record.plan.iter().take(100) {
            lines.push(format!("{} {:?} · {}", step.id, step.state, step.text));
            if let Some(evidence) = &step.evidence {
                lines.push(format!("  Evidence: {evidence}"));
            }
        }
    }
    lines
}

fn state_label(state: State) -> &'static str {
    match state {
        State::Starting => "starting",
        State::Running => "running",
        State::Completed => "finished",
        State::Failed => "failed",
        State::Interrupted => "interrupted",
        State::Cancelled => "stopped",
        State::Applied => "applied",
        State::Discarded => "discarded",
    }
}

fn elapsed_label(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    if seconds >= 3_600 {
        format!("{}h {:02}m", seconds / 3_600, (seconds / 60) % 60)
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

fn safe(text: &str) -> String {
    crate::commands::display_safe(&crate::redact::redact(text))
}

fn view_text(text: &str) -> String {
    let text = safe(text);
    if crate::ui::stdout_glyphs().focus() == ">" {
        text.replace(" · ", " | ")
            .replace("↑/↓", "Up/Down")
            .replace('…', "...")
    } else {
        text
    }
}

fn controls(record: &Record) -> Vec<(char, &'static str, Control)> {
    let mut choices = Vec::new();
    if matches!(record.state, State::Starting | State::Running) {
        choices.push(('c', "stop", Control::Stop));
    } else if !matches!(record.state, State::Applied | State::Discarded) {
        if matches!(
            record.state,
            State::Failed | State::Interrupted | State::Cancelled
        ) {
            choices.push(('r', "resume", Control::Resume));
        }
        if record.native_task_id.is_some() {
            choices.push(('s', "rework", Control::Rework));
        }
        if record.worktree.is_some() {
            if !record.budget_exceeded
                && matches!(
                    record.state,
                    State::Completed | State::Failed | State::Interrupted
                )
            {
                choices.push(('a', "apply", Control::Apply));
            }
            choices.push(('d', "discard", Control::Discard));
        }
    }
    choices
}

fn control_key(record: &Record, key: char) -> Option<Control> {
    controls(record)
        .into_iter()
        .find(|(letter, _, _)| *letter == key)
        .map(|(_, _, control)| control)
}

fn run_control(config: &Config, record: &Record, control: Control) -> Result<()> {
    let id = record.id.clone();
    let objective = crate::ui::truncate_cells(&safe(&record.objective), 100);
    promptui::section(&objective);
    let (question, action) = match control {
        Control::Stop => (
            "Stop this task? Its checkpoint and work are retained.",
            Action::Cancel { id },
        ),
        Control::Resume => (
            "Resume this task using its saved scope, model, and remaining budget?",
            Action::Resume { id },
        ),
        Control::Apply => (
            "Apply all task changes to the source repository? Review changes first.",
            Action::Apply {
                id,
                hunks: Vec::new(),
            },
        ),
        Control::Discard => (
            "Discard this task's isolated worktree and its unapplied changes?",
            Action::Discard { id },
        ),
        Control::Rework => {
            let Some(instructions) =
                promptui::text("Rework instructions", "Describe what to change", |value| {
                    if value.trim().is_empty() || value.len() > 32 * 1024 {
                        anyhow::bail!("instructions must contain 1..=32768 bytes");
                    }
                    Ok(())
                })?
            else {
                return Ok(());
            };
            if instructions == ":back" {
                return Ok(());
            }
            (
                "Restart this task with these instructions and its remaining budget?",
                Action::Rework { id, instructions },
            )
        }
    };
    if promptui::confirm(question, false)? == Some(true) {
        background::command(config, action)?;
    }
    Ok(())
}

fn print_snapshot(browser: &Browser, page: Page) {
    println!("Background tasks");
    if page == Page::Details {
        for line in page_body(browser, page) {
            println!("{}", view_text(&line));
        }
    } else if browser.entries.is_empty() {
        println!("No background tasks.");
    } else {
        for entry in &browser.entries {
            println!(
                "{}  {}  {}  {}",
                entry.id,
                state_label(entry.state),
                elapsed_label(entry.elapsed_ms),
                safe(&entry.objective)
            );
        }
    }
}

fn browse_static(
    config: &Config,
    project: &Path,
    seen: Option<&Path>,
    mut browser: Browser,
    mut page: Page,
) -> Result<u8> {
    loop {
        if page == Page::List {
            let mut labels = browser
                .entries
                .iter()
                .map(|entry| {
                    view_text(&format!(
                        "{} · {} · {}",
                        state_label(entry.state),
                        elapsed_label(entry.elapsed_ms),
                        entry.objective
                    ))
                })
                .collect::<Vec<_>>();
            labels.extend([
                "Refresh tasks".into(),
                "Switch project / all projects".into(),
                "Close".into(),
            ]);
            match promptui::filter_picker("Background tasks", &labels, 0)? {
                promptui::PickerResult::Use(index) if index < browser.entries.len() => {
                    let id = browser.entries[index].id.clone();
                    browser.open(&id, seen)?;
                    page = Page::Details;
                }
                promptui::PickerResult::Use(index) if index == browser.entries.len() => {
                    browser.refresh(project, true)?
                }
                promptui::PickerResult::Use(index) if index == browser.entries.len() + 1 => {
                    browser.all = !browser.all;
                    browser.refresh(project, true)?;
                }
                _ => return Ok(0),
            }
        } else {
            promptui::section("Task details");
            for line in page_body(&browser, page) {
                promptui::note(&view_text(&line));
            }
            let record = &browser
                .details
                .as_ref()
                .context("selected task unavailable")?
                .record;
            let controls = controls(record);
            let mut labels = vec!["View activity".into(), "View changes".into()];
            labels.extend(controls.iter().map(|(_, label, _)| label.to_string()));
            labels.push("Back to tasks".into());
            match promptui::filter_picker("Task actions", &labels, 0)? {
                promptui::PickerResult::Use(0) => {
                    promptui::section("Task activity");
                    for line in &browser.details.as_ref().unwrap().log_lines {
                        promptui::note(&safe(line));
                    }
                }
                promptui::PickerResult::Use(1) => {
                    promptui::section("Task changes");
                    for line in background::task_patch_lines(&record.id, 2_000)? {
                        promptui::note(&safe(&line));
                    }
                }
                promptui::PickerResult::Use(index) if index < controls.len() + 2 => {
                    run_control(config, record, controls[index - 2].2)?;
                    let id = record.id.clone();
                    browser.refresh(project, true)?;
                    browser.open(&id, seen)?;
                }
                _ => page = Page::List,
            }
        }
    }
}
