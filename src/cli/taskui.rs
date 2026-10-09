//! A small live task drawer. Observation is read-only; controls leave raw mode
//! and require a separate, explicit choice before using the task engine.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::background::{
    self, Action, EntryQuery, FollowupStatus, InteractionKind, State, TaskDetails, TaskEntry,
};
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
    Evidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Control {
    Stop,
    Resume,
    Rework,
    Apply,
    Discard,
    Respond,
    Followup,
    Queue,
    Rename,
    Pin,
    Archive,
    Views,
    Actions,
}

#[derive(Default)]
struct Browser {
    entries: Vec<TaskEntry>,
    query: String,
    selected_id: Option<String>,
    all: bool,
    needs_you: bool,
    archived: bool,
    details: Option<TaskDetails>,
    patch: Vec<String>,
    scroll: usize,
    notice: Option<String>,
}

/// Non-terminal callers receive a bounded readable snapshot rather than an
/// interactive prompt. No provider, MCP connection or worker is initialized.
pub fn browse(config: &Config, id: Option<&str>, all: bool) -> Result<u8> {
    browse_filtered(config, id, all, false, false)
}

pub fn browse_filtered(
    config: &Config,
    id: Option<&str>,
    all: bool,
    needs_you: bool,
    archived: bool,
) -> Result<u8> {
    let project = std::env::current_dir().context("reading task browser project")?;
    let seen = std::env::var_os("AISHE_BACKGROUND_SEEN_FILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let mut browser = Browser {
        all,
        needs_you,
        archived,
        ..Browser::default()
    };
    browser.refresh(&project, true)?;
    // A global badge must never open an apparently empty list just because the
    // work belongs to another project. Keep the scope visible in the drawer.
    if !all && browser.entries.is_empty() {
        let elsewhere = background::cached_task_entries_with_query(None, browser.entry_query())?;
        if !elsewhere.is_empty() {
            browser.all = true;
            browser.entries = elsewhere;
            browser.filter_entries();
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
                let control = if control == Control::Actions {
                    let Some(details) = &browser.details else {
                        continue;
                    };
                    let choices = controls(details);
                    let mut labels = choices
                        .iter()
                        .map(|(_, label, _)| label.to_string())
                        .collect::<Vec<_>>();
                    labels.push("Back to task".into());
                    let promptui::PickerResult::Use(index) =
                        promptui::filter_picker("Task actions", &labels, labels.len() - 1)?
                    else {
                        continue;
                    };
                    let Some((_, _, control)) = choices.get(index) else {
                        continue;
                    };
                    *control
                } else {
                    control
                };
                if control == Control::Views {
                    choose_view(&mut browser)?;
                    browser.refresh(&project, true)?;
                    page = Page::List;
                    continue;
                }
                let Some(details) = &browser.details else {
                    continue;
                };
                let task_id = details.record.id.clone();
                // The interactive frame and raw guard have both been dropped.
                // Reading alone can never spawn, signal, apply or discard work.
                if let Err(error) = run_control(config, details, control) {
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
            background::task_entries_with_query(project, self.entry_query())?
        } else {
            background::cached_task_entries_with_query(project, self.entry_query())?
        };
        self.filter_entries();
        let matches = self.matches();
        if !matches
            .iter()
            .any(|entry| Some(&entry.id) == self.selected_id.as_ref())
        {
            self.selected_id = matches.first().map(|entry| entry.id.clone());
        }
        Ok(())
    }

    fn entry_query(&self) -> EntryQuery {
        EntryQuery {
            include_closed: self.archived,
            include_archived: self.archived,
        }
    }

    fn filter_entries(&mut self) {
        self.entries.retain(|entry| {
            (!self.needs_you || entry.pending_requests > 0) && (!self.archived || entry.archived)
        });
    }

    fn matches(&self) -> Vec<&TaskEntry> {
        let words = self.query.to_lowercase();
        self.entries
            .iter()
            .filter(|entry| {
                let text = format!(
                    "{} {} {} {} {}",
                    entry.id,
                    entry.title,
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
        if !matches!(
            details.record.state,
            State::Starting | State::Running | State::Waiting
        ) {
            if let Some(path) = seen {
                background::acknowledge_task(&details.entry, path)?;
            } else {
                background::mark_task_reviewed(&details.entry)?;
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
                *page = if matches!(*page, Page::Activity | Page::Changes | Page::Evidence) {
                    Page::Details
                } else {
                    Page::List
                };
                browser.scroll = 0;
                browser.notice = None;
            }
            PickerKey::Character('\x16') if *page == Page::List => return Ok(Some(Control::Views)),
            PickerKey::Enter
                if *page == Page::Details
                    && browser
                        .details
                        .as_ref()
                        .is_some_and(|d| !d.interaction.pending.is_empty()) =>
            {
                return Ok(Some(Control::Respond))
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
            PickerKey::Character('e') if *page == Page::Details => {
                *page = Page::Evidence;
                browser.scroll = 0;
            }
            PickerKey::Character('l') if *page == Page::Details => {
                *page = Page::Activity;
                browser.scroll = 0;
            }
            PickerKey::Character('?') if *page == Page::Details => {
                return Ok(Some(Control::Actions))
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
                    if let Some(control) = control_key(details, c) {
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
        Page::List if browser.needs_you => "Needs you",
        Page::List if browser.archived => "Archived tasks",
        Page::List => "Background tasks",
        Page::Details => "Task details",
        Page::Activity => "Task activity",
        Page::Changes => "Task changes",
        Page::Evidence => "Recorded checks",
    };
    let mut lines = vec![format!("  {title}")];
    if budget == 1 {
        return fit_lines(lines, width, budget);
    }
    let footer = if page == Page::List {
        "Enter details · Tab project/all · Ctrl-V views · Esc close".to_string()
    } else if page == Page::Details {
        if browser
            .details
            .as_ref()
            .is_some_and(|d| !d.interaction.pending.is_empty())
        {
            "Enter respond · f follow-up · e checks · ? actions · Esc back".into()
        } else if browser
            .details
            .as_ref()
            .is_some_and(|d| matches!(d.record.state, State::Starting | State::Running))
        {
            "f follow-up · e checks · l activity · ? actions · Esc back".into()
        } else if browser
            .details
            .as_ref()
            .is_some_and(|details| details.record.worktree.is_some())
        {
            "e checks · l activity · p changes · ? actions · Esc back".into()
        } else {
            "e checks · l activity · ? actions · Esc back".into()
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
            lines.push(if browser.needs_you && browser.query.is_empty() {
                "  Inbox zero. No tasks need your response.".into()
            } else if browser.archived && browser.query.is_empty() {
                "  No archived tasks. Archive finished work from its actions menu.".into()
            } else if browser.query.is_empty() {
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
                    entry_state(entry),
                    elapsed_label(entry.elapsed_ms),
                    format_args!(
                        "{}{}",
                        if entry.pinned { "[pin] " } else { "" },
                        entry.title
                    )
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
        let available = body_viewport(browser, page, size);
        let offset = browser.scroll.min(body.len().saturating_sub(available));
        lines.extend(
            body.into_iter()
                .skip(offset)
                .take(available)
                .map(|line| format!("  {line}")),
        );
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
        Page::Evidence => evidence_lines(details),
        _ => detail_lines(details, browser.notice.as_deref()),
    }
}

fn body_rows(browser: &Browser, page: Page, width: usize) -> Vec<String> {
    page_body(browser, page)
        .into_iter()
        .flat_map(|line| crate::ui::wrap_cells(&view_text(&line), width.saturating_sub(2).max(1)))
        .collect()
}

fn body_viewport(_browser: &Browser, _page: Page, size: (usize, usize)) -> usize {
    size.1
        .saturating_sub(1)
        .clamp(1, MAX_FRAME_ROWS)
        .saturating_sub(2)
        .max(1)
}

fn detail_lines(details: &TaskDetails, notice: Option<&str>) -> Vec<String> {
    let record = &details.record;
    let mut lines = vec![
        details.entry.title.clone(),
        format!(
            "{} · {} · {}",
            state_label(record.state),
            elapsed_label(details.elapsed_ms()),
            record.id
        ),
        format!("Last activity: {}", details.activity),
    ];
    if details.entry.title != record.objective {
        lines.push(format!("Objective: {}", record.objective));
    }
    for request in &details.interaction.pending {
        lines.push(
            match request.kind {
                InteractionKind::Question => "Needs you: question · Enter to answer",
                InteractionKind::Approval => "Needs you: specific action · Enter to review",
            }
            .into(),
        );
        lines.extend(request.prompt.lines().take(30).map(ToOwned::to_owned));
        if request.kind == InteractionKind::Approval {
            lines.push(format!(
                "Action: {} · {} · network {}",
                request.binding.tool_name,
                scope_label(request.binding.scope),
                network_label(request.binding.network)
            ));
            lines.push(format!("Directory: {}", request.binding.cwd.display()));
        }
    }
    if details.interaction.queued > 0 || details.interaction.received > 0 {
        lines.push(format!(
            "Follow-ups: {} queued · {} received · f to steer",
            details.interaction.queued, details.interaction.received
        ));
        for followup in details
            .interaction
            .followups
            .iter()
            .rev()
            .filter(|f| f.status != FollowupStatus::Removed)
            .take(4)
        {
            lines.push(format!(
                "#{} {} · {}",
                followup.revision,
                followup_label(followup.status),
                followup.text
            ));
        }
    }
    if let Some(checkpoint) = &details.checkpoint {
        lines.push(format!(
            "{} · e to inspect",
            checkpoint.check_summary.label()
        ));
        if details.interaction.pending.is_empty()
            && (!matches!(record.state, State::Starting | State::Running)
                || checkpoint.check_summary.total > 0)
        {
            for unresolved in checkpoint.check_summary.unresolved.iter().take(6) {
                lines.push(format!("Unresolved: {unresolved}"));
            }
        }
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
        if matches!(
            record.state,
            State::Failed | State::Interrupted | State::Cancelled
        ) {
            if let Some(error) = &checkpoint.last_error {
                lines.push(format!("Checkpoint: {error}"));
            }
        }
    }
    if details.interaction.pending.is_empty() {
        if let Some(error) = &record.error {
            lines.push(format!("Attention: {error}"));
        }
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
                lines.push(format!("  Plan note: {evidence}"));
            }
        }
    }
    lines
}

fn state_label(state: State) -> &'static str {
    match state {
        State::Starting => "starting",
        State::Running => "running",
        State::Waiting => "waiting",
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

fn controls(details: &TaskDetails) -> Vec<(char, &'static str, Control)> {
    let record = &details.record;
    let mut choices = Vec::new();
    if !details.interaction.pending.is_empty() {
        choices.push(('u', "respond", Control::Respond));
    }
    if matches!(
        record.state,
        State::Starting | State::Running | State::Waiting
    ) {
        choices.push(('f', "follow-up", Control::Followup));
        choices.push(('c', "stop", Control::Stop));
        if record.state == State::Waiting && details.interaction.pending.is_empty() {
            choices.push(('r', "resume", Control::Resume));
        }
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
    if details.interaction.queued > 0 {
        choices.push(('q', "queued follow-ups", Control::Queue));
    }
    choices.push(('n', "rename", Control::Rename));
    choices.push((
        'i',
        if details.entry.pinned { "unpin" } else { "pin" },
        Control::Pin,
    ));
    if !matches!(
        record.state,
        State::Starting | State::Running | State::Waiting
    ) {
        choices.push((
            'h',
            if details.entry.archived {
                "unarchive"
            } else {
                "archive"
            },
            Control::Archive,
        ));
    }
    choices
}

fn control_key(details: &TaskDetails, key: char) -> Option<Control> {
    controls(details)
        .into_iter()
        .find(|(letter, _, _)| *letter == key)
        .map(|(_, _, control)| control)
}

fn choose_view(browser: &mut Browser) -> Result<()> {
    let labels = ["Current work", "Needs you", "Archived history"].map(String::from);
    let current = if browser.needs_you {
        1
    } else if browser.archived {
        2
    } else {
        0
    };
    if let promptui::PickerResult::Use(index) =
        promptui::filter_picker("Task views", &labels, current)?
    {
        browser.needs_you = index == 1;
        browser.archived = index == 2;
        browser.query.clear();
        browser.selected_id = None;
    }
    Ok(())
}

fn read_message(label: &str, default: &str) -> Result<Option<String>> {
    Ok(promptui::text(label, default, |value| {
        if value.trim().is_empty() || value.len() > 16 * 1024 {
            anyhow::bail!("Enter 1..=16384 bytes");
        }
        Ok(())
    })?
    .filter(|value| value != ":back"))
}

fn respond(config: &Config, details: &TaskDetails) -> Result<()> {
    let Some(request) = details.interaction.pending.first() else {
        return Ok(());
    };
    promptui::section("Needs you");
    for line in request.prompt.lines() {
        promptui::note(&safe(line));
    }
    let id = details.record.id.clone();
    let request_id = request.id.clone();
    let action = match request.kind {
        InteractionKind::Question => {
            let answer = if request.choices.is_empty() {
                read_message("Answer question", "")?
            } else {
                let mut labels = request.choices.clone();
                labels.extend(["Write an answer".into(), "Leave for later".into()]);
                match promptui::filter_picker("Answer question", &labels, labels.len() - 1)? {
                    promptui::PickerResult::Use(index) if index < request.choices.len() => {
                        Some(request.choices[index].clone())
                    }
                    promptui::PickerResult::Use(index) if index == request.choices.len() => {
                        read_message("Answer question", "")?
                    }
                    _ => None,
                }
            };
            let Some(text) = answer else {
                return Ok(());
            };
            Action::Answer {
                id,
                request_id,
                text,
            }
        }
        InteractionKind::Approval => {
            promptui::key_value("action", &safe(&request.binding.tool_name));
            promptui::key_value("directory", &safe(&request.binding.cwd.to_string_lossy()));
            promptui::key_value(
                "scope",
                &format!(
                    "{} · network {}",
                    scope_label(request.binding.scope),
                    network_label(request.binding.network)
                ),
            );
            promptui::note("This decision covers only this exact action. The task keeps its scope and remaining budget.");
            let labels = [
                "Approve this exact action",
                "Deny and continue",
                "Leave for later",
            ]
            .map(String::from);
            match promptui::filter_picker("Specific action approval", &labels, 2)? {
                promptui::PickerResult::Use(0) => Action::Approve { id, request_id },
                promptui::PickerResult::Use(1) => {
                    let Some(reason) = read_message("Reason (optional)", "Denied by user")? else {
                        return Ok(());
                    };
                    Action::Deny {
                        id,
                        request_id,
                        reason,
                    }
                }
                _ => return Ok(()),
            }
        }
    };
    background::command(config, action)?;
    Ok(())
}

fn manage_queue(config: &Config, details: &TaskDetails) -> Result<()> {
    let queued = details
        .interaction
        .followups
        .iter()
        .filter(|f| f.status == FollowupStatus::Queued)
        .collect::<Vec<_>>();
    if queued.is_empty() {
        return Ok(());
    }
    let mut labels = queued
        .iter()
        .map(|f| safe(&format!("#{} · {}", f.revision, f.text)))
        .collect::<Vec<_>>();
    labels.push("Back".into());
    let promptui::PickerResult::Use(index) =
        promptui::filter_picker("Queued follow-ups", &labels, labels.len() - 1)?
    else {
        return Ok(());
    };
    let Some(followup) = queued.get(index) else {
        return Ok(());
    };
    let options = ["Edit queued message", "Remove queued message", "Back"].map(String::from);
    let id = details.record.id.clone();
    let revision = followup.revision;
    let action = match promptui::filter_picker("Follow-up", &options, 2)? {
        promptui::PickerResult::Use(0) => {
            let Some(text) = read_message("Follow-up instructions", &followup.text)? else {
                return Ok(());
            };
            Action::EditFollowup { id, revision, text }
        }
        promptui::PickerResult::Use(1) => Action::RemoveFollowup { id, revision },
        _ => return Ok(()),
    };
    background::command(config, action)?;
    Ok(())
}

fn run_control(config: &Config, details: &TaskDetails, control: Control) -> Result<()> {
    let record = &details.record;
    let id = record.id.clone();
    promptui::section(&crate::ui::truncate_cells(&safe(&details.entry.title), 100));
    let (question, action) = match control {
        Control::Respond => return respond(config, details),
        Control::Queue => return manage_queue(config, details),
        Control::Followup => {
            let Some(text) = read_message("Follow-up instructions", "")? else {
                return Ok(());
            };
            background::command(config, Action::Followup { id, text })?;
            return Ok(());
        }
        Control::Rename => {
            let Some(name) =
                read_message("Task name (up to 120 characters)", &details.entry.title)?
            else {
                return Ok(());
            };
            background::rename_task(&id, &name)?;
            return Ok(());
        }
        Control::Pin => {
            background::pin_task(&id, !details.entry.pinned)?;
            return Ok(());
        }
        Control::Archive if details.entry.archived => {
            background::archive_task(&id, false)?;
            return Ok(());
        }
        Control::Archive => (
            "Archive this finished task? Its result and workspace are retained.",
            Action::Archive { id, archived: true },
        ),
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
            let Some(instructions) = read_message("Rework instructions", "")? else {
                return Ok(());
            };
            (
                "Restart this task with these instructions and its remaining budget?",
                Action::Rework { id, instructions },
            )
        }
        Control::Views | Control::Actions => return Ok(()),
    };
    if promptui::confirm(question, false)? == Some(true) {
        background::command(config, action)?;
    }
    Ok(())
}

fn scope_label(scope: crate::agent::ExecutionScope) -> &'static str {
    match scope {
        crate::agent::ExecutionScope::Workspace => "workspace",
        crate::agent::ExecutionScope::Host => "host",
    }
}
fn network_label(network: crate::agent::NetworkPolicy) -> &'static str {
    match network {
        crate::agent::NetworkPolicy::Deny => "deny",
        crate::agent::NetworkPolicy::Allow => "allow",
    }
}

fn entry_state(entry: &TaskEntry) -> &'static str {
    if entry.pending_requests > 0 {
        "needs you"
    } else {
        state_label(entry.state)
    }
}

fn followup_label(status: FollowupStatus) -> &'static str {
    match status {
        FollowupStatus::Queued => "queued",
        FollowupStatus::Received => "received",
        FollowupStatus::Removed => "removed",
    }
}

fn evidence_lines(details: &TaskDetails) -> Vec<String> {
    use crate::tasks::{ExecutionKind, ExecutionOutcome};
    let Some(checkpoint) = &details.checkpoint else {
        return vec!["No recorded checks.".into()];
    };
    let mut lines = vec![checkpoint.check_summary.label(), "Only actual command outcomes are shown. Earlier checks become stale after later commands or file changes.".into()];
    for unresolved in &checkpoint.check_summary.unresolved {
        lines.push(format!("Unresolved: {unresolved}"));
    }
    for check in checkpoint
        .evidence
        .iter()
        .rev()
        .filter(|e| e.kind == ExecutionKind::Check)
    {
        let outcome = match check.outcome {
            ExecutionOutcome::Running => "running",
            ExecutionOutcome::Passed => "passed",
            ExecutionOutcome::Failed => "failed",
            ExecutionOutcome::Cancelled => "cancelled",
            ExecutionOutcome::NotRun => "not run",
            ExecutionOutcome::Uncertain => "uncertain",
        };
        lines.push(format!(
            "{}{} · exit {} · {}ms · {}",
            outcome,
            if check.stale(checkpoint.workspace_revision) {
                " (stale)"
            } else {
                ""
            },
            check
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unrecorded".into()),
            check
                .duration_ms
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".into()),
            check.command
        ));
        lines.push(format!("Directory: {}", check.cwd.display()));
        lines.push(format!("Started {}", recorded_time(check.started_at_ms)));
        if let Some(finished) = check.finished_at_ms {
            lines.push(format!("Finished {}", recorded_time(finished)));
        }
        lines.extend(check.output.lines().map(ToOwned::to_owned));
    }
    if checkpoint.evidence_dropped > 0 {
        lines.push(format!(
            "{} older execution records omitted by the history limit.",
            checkpoint.evidence_dropped
        ));
    }
    lines
}

fn recorded_time(milliseconds: u128) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let seconds = now.saturating_sub(milliseconds) / 1000;
    if seconds < 60 {
        "just now".into()
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86400)
    }
}

fn print_snapshot(browser: &Browser, page: Page) {
    println!(
        "{}",
        if browser.needs_you {
            "Needs you"
        } else if browser.archived {
            "Archived tasks"
        } else {
            "Background tasks"
        }
    );
    if page == Page::Details {
        for line in page_body(browser, page) {
            println!("{}", view_text(&line));
        }
    } else if browser.entries.is_empty() {
        println!(
            "{}",
            if browser.needs_you {
                "Inbox zero. No tasks need your response."
            } else if browser.archived {
                "No archived tasks."
            } else {
                "No background tasks."
            }
        );
    } else {
        for entry in &browser.entries {
            println!(
                "{}  {}  {}  {}",
                entry.id,
                entry_state(entry),
                elapsed_label(entry.elapsed_ms),
                safe(&entry.title)
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
                        entry_state(entry),
                        elapsed_label(entry.elapsed_ms),
                        entry.title
                    ))
                })
                .collect::<Vec<_>>();
            labels.extend([
                "Refresh tasks".into(),
                "Switch project / all projects".into(),
                "Task views".into(),
                "Close".into(),
            ]);
            let title = if browser.needs_you {
                "Needs you"
            } else if browser.archived {
                "Archived tasks"
            } else {
                "Background tasks"
            };
            match promptui::filter_picker(title, &labels, 0)? {
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
                promptui::PickerResult::Use(index) if index == browser.entries.len() + 2 => {
                    choose_view(&mut browser)?;
                    browser.refresh(project, true)?;
                }
                _ => return Ok(0),
            }
        } else {
            promptui::section("Task details");
            for line in page_body(&browser, page) {
                promptui::note(&view_text(&line));
            }
            let details = browser
                .details
                .as_ref()
                .context("selected task unavailable")?;
            let record = &details.record;
            let controls = controls(details);
            let mut labels = vec![
                "View activity".into(),
                "View changes".into(),
                "Recorded checks".into(),
            ];
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
                promptui::PickerResult::Use(2) => {
                    promptui::section("Recorded checks");
                    for line in evidence_lines(details) {
                        promptui::note(&safe(&line));
                    }
                }
                promptui::PickerResult::Use(index) if index < controls.len() + 3 => {
                    run_control(config, details, controls[index - 3].2)?;
                    let id = record.id.clone();
                    browser.refresh(project, true)?;
                    browser.open(&id, seen)?;
                }
                _ => page = Page::List,
            }
        }
    }
}
