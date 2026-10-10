//! A small live task drawer. Observation is read-only; controls leave raw mode
//! and require a separate, explicit choice before using the task engine.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use unicode_segmentation::UnicodeSegmentation;

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
    Timeline,
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
    Reviewed,
    Views,
    Actions,
    Foreground,
    TimelineFilter,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum TimelineFilter {
    #[default]
    All,
    Tools,
    Checks,
    Interactions,
    Handoffs,
    Workflow,
}

impl TimelineFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All events",
            Self::Tools => "Tools",
            Self::Checks => "Checks",
            Self::Interactions => "Questions and follow-ups",
            Self::Handoffs => "Handoffs",
            Self::Workflow => "Workflow",
        }
    }

    fn includes(self, kind: crate::tasks::EventKind) -> bool {
        use crate::tasks::EventKind;
        match self {
            Self::All => true,
            Self::Tools => matches!(
                kind,
                EventKind::ToolPlanned | EventKind::ToolStarted | EventKind::ToolResult
            ),
            Self::Checks => kind == EventKind::CheckResult,
            Self::Interactions => matches!(
                kind,
                EventKind::Question
                    | EventKind::Approval
                    | EventKind::FollowupQueued
                    | EventKind::FollowupReceived
            ),
            Self::Handoffs => matches!(
                kind,
                EventKind::HandoffRequested | EventKind::HandoffCompleted | EventKind::Resumed
            ),
            Self::Workflow => matches!(
                kind,
                EventKind::WorkflowQueued | EventKind::WorkflowReleased | EventKind::PlanNote
            ),
        }
    }
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
    timeline_filter: TimelineFilter,
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
                if control == Control::TimelineFilter {
                    choose_timeline_filter(&mut browser)?;
                    browser.scroll = 0;
                    page = Page::Timeline;
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
        let matches = self
            .entries
            .iter()
            .filter(|entry| {
                let text = format!(
                    "{} {} {} {} {} {} {} {}",
                    entry.id,
                    entry.title,
                    entry.objective,
                    state_label(entry.state),
                    entry.project.display(),
                    entry.stage_name.as_deref().unwrap_or(""),
                    entry.stage_key.as_deref().unwrap_or(""),
                    entry.workflow_id.as_deref().unwrap_or("")
                )
                .to_lowercase();
                words.split_whitespace().all(|word| text.contains(word))
            })
            .collect::<Vec<_>>();
        // Use the same group order for arrow movement and rendered rows.
        let mut groups = std::collections::BTreeSet::new();
        let mut ordered = Vec::with_capacity(matches.len());
        for entry in &matches {
            if let Some(workflow) = &entry.workflow_id {
                if groups.insert(workflow.as_str()) {
                    ordered.extend(
                        matches
                            .iter()
                            .copied()
                            .filter(|stage| stage.workflow_id.as_ref() == Some(workflow)),
                    );
                }
            } else {
                ordered.push(*entry);
            }
        }
        ordered
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
            State::Starting | State::Running | State::Waiting | State::Blocked
        ) {
            if let Some(path) = seen {
                background::acknowledge_task(&details.entry, path)?;
            } else {
                background::mark_task_seen(&details.entry)?;
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
                *page = if matches!(
                    *page,
                    Page::Activity | Page::Changes | Page::Evidence | Page::Timeline
                ) {
                    Page::Details
                } else {
                    Page::List
                };
                browser.scroll = 0;
                browser.notice = None;
            }
            PickerKey::Character('\x16') if *page == Page::List => return Ok(Some(Control::Views)),
            PickerKey::Character('\x06') if *page == Page::Timeline => {
                return Ok(Some(Control::TimelineFilter))
            }
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
                            browser.patch = change_lines(&id)
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
            PickerKey::Character('t') if *page == Page::Details => {
                *page = Page::Timeline;
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
                    browser.patch = change_lines(&details.record.id)
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
        Page::Timeline => "Task timeline",
    };
    let body = (page != Page::List).then(|| body_rows(browser, page, width));
    let available = body_viewport(browser, page, size);
    let offset = body.as_ref().map_or(0, |body| {
        browser.scroll.min(body.len().saturating_sub(available))
    });
    let mut lines = vec![if let Some(body) = &body {
        let end = (offset + available).min(body.len());
        format!(
            "  {title} · {}-{end}/{}{}",
            offset + 1,
            body.len(),
            if end < body.len() {
                " · more below"
            } else {
                ""
            }
        )
    } else {
        format!("  {title}")
    }];
    if budget == 1 {
        return fit_lines(lines, width, budget);
    }
    let footer = if page != Page::List && width < 58 {
        "↑/↓ · Esc back · ?".to_string()
    } else if page == Page::List {
        "Enter details · Tab project/all · Ctrl-V views · Esc close".to_string()
    } else if page == Page::Details {
        if browser
            .details
            .as_ref()
            .is_some_and(|d| !d.interaction.pending.is_empty())
        {
            "↑/↓ scroll · Enter respond · f steer · ? actions · Esc back".into()
        } else if browser
            .details
            .as_ref()
            .is_some_and(|d| matches!(d.record.state, State::Starting | State::Running))
        {
            "↑/↓ scroll · f steer · e checks · ? actions · Esc back".into()
        } else if browser
            .details
            .as_ref()
            .is_some_and(|details| details.record.worktree.is_some())
        {
            "↑/↓ scroll · e checks · p changes · ? actions · Esc back".into()
        } else {
            "↑/↓ scroll · e checks · ? actions · Esc back".into()
        }
    } else if page == Page::Timeline {
        "↑/↓ scroll · Ctrl-F filter · Ctrl-R refresh · Esc back".into()
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
        let reserve_project = usize::from(budget > 7 && browser.all);
        let available = budget
            .saturating_sub(lines.len() + 1 + reserve_project)
            .max(1);
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
            let rows = task_rows(&entries, browser.selected_id.as_deref());
            let selected = rows
                .iter()
                .position(|(id, _)| *id == browser.selected_id.as_deref())
                .unwrap_or(0);
            let start = selected
                .saturating_sub(available / 2)
                .min(rows.len().saturating_sub(available));
            lines.extend(
                rows.into_iter()
                    .skip(start)
                    .take(available)
                    .map(|(_, line)| line),
            );
            if budget > 7 && browser.all {
                if let Some(entry) = entries
                    .iter()
                    .find(|entry| Some(entry.id.as_str()) == browser.selected_id.as_deref())
                {
                    let project_line = format!("  project: {}", entry.project.display());
                    lines.push(project_line);
                }
            }
        }
    } else {
        lines.extend(
            body.unwrap_or_default()
                .into_iter()
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

fn task_rows<'a>(
    entries: &[&'a TaskEntry],
    selected: Option<&str>,
) -> Vec<(Option<&'a str>, String)> {
    let mut groups = std::collections::BTreeSet::new();
    let mut rows = Vec::new();
    for entry in entries {
        if let Some(workflow) = &entry.workflow_id {
            if !groups.insert(workflow.as_str()) {
                continue;
            }
            rows.push((
                None,
                format!(
                    "  Workflow {} · {}/{} finished",
                    workflow.chars().take(8).collect::<String>(),
                    entry.workflow_completed,
                    entry.workflow_total
                ),
            ));
            for stage in entries
                .iter()
                .filter(|stage| stage.workflow_id.as_deref() == Some(workflow))
            {
                rows.push((
                    Some(stage.id.as_str()),
                    format!(
                        "    {} {} · {} · {}",
                        if selected == Some(stage.id.as_str()) {
                            ">"
                        } else {
                            " "
                        },
                        entry_state(stage),
                        elapsed_label(stage.elapsed_ms),
                        stage.stage_name.as_deref().unwrap_or(&stage.title)
                    ),
                ));
            }
        } else {
            rows.push((
                Some(entry.id.as_str()),
                format!(
                    "  {} {} · {} · {}{}",
                    if selected == Some(entry.id.as_str()) {
                        ">"
                    } else {
                        " "
                    },
                    entry_state(entry),
                    elapsed_label(entry.elapsed_ms),
                    if entry.pinned { "[pin] " } else { "" },
                    entry.title
                ),
            ));
        }
    }
    rows
}

fn change_lines(id: &str) -> Result<Vec<String>> {
    Ok(crate::cli::changeui::review_lines(
        &background::task_change_review(id)?,
    ))
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
        Page::Timeline => timeline_lines(details, browser.timeline_filter),
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
    let limits = display_limits(details);
    let bounded = limits.provider_turns.is_some()
        && limits.tool_calls.is_some()
        && limits.network_calls.is_some()
        && limits.elapsed.is_some();
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
    if let Some(request) = details.interaction.pending.first() {
        lines.push(
            match request.kind {
                InteractionKind::Question => "Needs you: question · Enter to answer",
                InteractionKind::Approval => "Needs you: specific action · Enter to review",
            }
            .into(),
        );
        lines.extend(
            request
                .prompt
                .lines()
                .take(2)
                .map(|line| crate::ui::truncate_cells(line, 80)),
        );
    }
    if details.interaction.queued > 0 || details.interaction.received > 0 {
        lines.push(format!(
            "Follow-ups: {} queued · {} received · f to steer",
            details.interaction.queued, details.interaction.received
        ));
    }
    lines.extend([
        format!(
            "Connection: {}",
            if record.connection_id.is_empty() {
                "unknown (older task)".into()
            } else {
                safe(&record.connection_id)
            }
        ),
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
    ]);
    if record.run_cwd != record.source_cwd {
        lines.push(format!("Workspace: {}", record.run_cwd.display()));
    }
    let count_cap = |value: Option<u32>| {
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "no task cap".into())
    };
    let time_cap = limits
        .elapsed
        .map(|duration| {
            if duration.as_secs().is_multiple_of(60) {
                format!("{}m", duration.as_secs() / 60)
            } else {
                format!("{}s", duration.as_secs())
            }
        })
        .unwrap_or_else(|| "no task cap".into());
    if bounded {
        lines.push(format!(
            "Time limit: {} · turns {} · tools {} · network {}",
            time_cap,
            count_cap(limits.provider_turns),
            count_cap(limits.tool_calls),
            count_cap(limits.network_calls)
        ));
    } else {
        lines.push(format!(
            "Limits: turns {} · tools {}",
            count_cap(limits.provider_turns),
            count_cap(limits.tool_calls)
        ));
        lines.push(format!(
            "Time: {} · network {}",
            time_cap,
            count_cap(limits.network_calls)
        ));
    }
    if let Some(cost) = limits.cost_usd {
        lines.push(format!("Cost limit: ${cost:.2}"));
    }
    if !details.entry.result_revision.is_empty() {
        lines.push(if details.entry.reviewed {
            "Review: reviewed by you".into()
        } else {
            "Review: seen · v to mark reviewed".into()
        });
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
    }
    if details.entry.title != record.objective {
        lines.push(format!("Objective: {}", record.objective));
    }
    if let Some(stage) = &record.workflow {
        lines.push(format!(
            "Workflow: {} · stage {}",
            stage.run_id, stage.stage_name
        ));
        if !stage.dependencies.is_empty() {
            lines.push(format!("Depends on: {}", stage.dependencies.join(", ")));
        }
        if !stage.required_checks.is_empty() {
            lines.push(format!(
                "Required checks: {}",
                stage.required_checks.join("; ")
            ));
        }
    }
    if let Some(handoff) = &details.handoff {
        use crate::agent::native::handoff::Status;
        match (handoff.status, handoff.requested) {
            (Status::Queued, Some(direction)) => lines.push(format!(
                "Handoff: queued to {} · waits for a safe execution boundary",
                direction.label()
            )),
            (Status::Parked, Some(direction)) => lines.push(format!(
                "Handoff: received · checkpoint parked for {} continuation",
                direction.label()
            )),
            (Status::Active, _) => lines.push(format!("Execution: {}", handoff.direction.label())),
            _ => {}
        }
    }
    for (index, request) in details.interaction.pending.iter().enumerate() {
        if index > 0
            || request.prompt.lines().nth(2).is_some()
            || request
                .prompt
                .lines()
                .take(2)
                .any(|line| crate::ui::cell_width(line) > 80)
        {
            lines.push("Full request · Enter to respond:".into());
            lines.extend(request.prompt.lines().take(30).map(ToOwned::to_owned));
        }
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
        let execution = checkpoint.execution;
        lines.push(if bounded {
            format!(
                "Used: {}/{} turns · {}/{} tools · {}/{} network",
                execution.provider_turns,
                limits.provider_turns.unwrap(),
                execution.tool_calls,
                limits.tool_calls.unwrap(),
                execution.network_calls,
                limits.network_calls.unwrap()
            )
        } else {
            format!(
                "Used: {} turns · {} tools · {} network",
                execution.provider_turns, execution.tool_calls, execution.network_calls
            )
        });
        lines.push(format!("Usage: {}", checkpoint.usage.tokens_label()));
        lines.push(format!("Cost: {}", execution.cost_label()));
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
    if record.state == State::Completed {
        lines.push("Finished by the agent; review its result and checks before applying.".into());
    }
    if let Some(result) = details
        .checkpoint
        .as_ref()
        .and_then(|c| c.latest_result.as_ref())
    {
        lines.push("Latest response · ↑/↓ scroll:".into());
        lines.extend(result.lines().take(150).map(ToOwned::to_owned));
        if result.lines().nth(150).is_some() {
            lines.push("Response continues in the saved task transcript.".into());
        }
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

fn display_limits(details: &TaskDetails) -> crate::agent::native::NativeLimits {
    if let Some(limits) = details
        .checkpoint
        .as_ref()
        .and_then(|checkpoint| checkpoint.execution_limits.as_ref())
    {
        return limits.clone();
    }
    // Compatibility for older checkpoints. The adoption ledger used sentinel
    // values where the native task had no explicit effect/time allowance.
    let budget = &details.record.budget;
    crate::agent::native::NativeLimits {
        provider_turns: (budget.max_provider_turns != u32::MAX)
            .then_some(budget.max_provider_turns),
        tool_calls: (budget.max_tool_calls != u32::MAX).then_some(budget.max_tool_calls),
        network_calls: (budget.max_network_calls != u32::MAX).then_some(budget.max_network_calls),
        elapsed: (budget.max_minutes != u32::MAX / 60)
            .then(|| Duration::from_secs(u64::from(budget.max_minutes) * 60)),
        cost_usd: (budget.max_cost_usd > 0.0).then_some(budget.max_cost_usd),
    }
}

fn state_label(state: State) -> &'static str {
    match state {
        State::Starting => "starting",
        State::Running => "running",
        State::Waiting => "waiting",
        State::Blocked => "queued",
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
            .replace("↳ ", "> ")
    } else {
        text
    }
}

fn controls(details: &TaskDetails) -> Vec<(char, &'static str, Control)> {
    let record = &details.record;
    let mut choices = Vec::new();
    if record.native_task_id.is_some()
        && !record.foreground
        && details.interaction.pending.is_empty()
        && matches!(
            record.state,
            State::Starting | State::Running | State::Waiting | State::Interrupted | State::Failed
        )
        && details
            .checkpoint
            .as_ref()
            .is_none_or(|checkpoint| checkpoint.status != crate::tasks::Status::Completed)
    {
        choices.push(('g', "continue in foreground", Control::Foreground));
    }
    if !details.interaction.pending.is_empty() {
        choices.push(('u', "respond", Control::Respond));
    }
    if record.state == State::Blocked {
        choices.push(('c', "stop queued stage", Control::Stop));
    } else if matches!(
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
    if !details.entry.reviewed && !details.entry.result_revision.is_empty() {
        choices.push(('v', "mark reviewed", Control::Reviewed));
    }
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

fn choose_timeline_filter(browser: &mut Browser) -> Result<()> {
    let filters = [
        TimelineFilter::All,
        TimelineFilter::Tools,
        TimelineFilter::Checks,
        TimelineFilter::Interactions,
        TimelineFilter::Handoffs,
        TimelineFilter::Workflow,
    ];
    let labels = filters
        .iter()
        .map(|filter| filter.label().to_string())
        .collect::<Vec<_>>();
    let selected = filters
        .iter()
        .position(|filter| *filter == browser.timeline_filter)
        .unwrap_or(0);
    if let promptui::PickerResult::Use(index) =
        promptui::filter_picker("Timeline filter", &labels, selected)?
    {
        if let Some(filter) = filters.get(index) {
            browser.timeline_filter = *filter;
        }
    }
    Ok(())
}

fn event_label(kind: crate::tasks::EventKind) -> &'static str {
    use crate::tasks::EventKind;
    match kind {
        EventKind::TaskStarted => "task started",
        EventKind::Admission => "admission",
        EventKind::ProviderTurn => "provider reservation",
        EventKind::ToolPlanned => "tool planned",
        EventKind::ToolStarted => "tool started",
        EventKind::ToolResult => "tool result",
        EventKind::Question => "question",
        EventKind::Approval => "specific approval",
        EventKind::FollowupQueued => "follow-up queued",
        EventKind::FollowupReceived => "follow-up received",
        EventKind::CheckResult => "recorded check",
        EventKind::HandoffRequested => "handoff requested",
        EventKind::HandoffCompleted => "handoff received",
        EventKind::Resumed => "resumed",
        EventKind::Finished => "attempt ended",
        EventKind::WorkflowQueued => "workflow queued",
        EventKind::WorkflowReleased => "workflow released",
        EventKind::PlanNote => "plan note",
    }
}

fn outcome_label(outcome: crate::tasks::EventOutcome) -> &'static str {
    use crate::tasks::EventOutcome;
    match outcome {
        EventOutcome::Completed => "completed",
        EventOutcome::Failed => "failed",
        EventOutcome::Cancelled => "cancelled",
        EventOutcome::Declined => "declined",
        EventOutcome::NotExecuted => "not executed",
        EventOutcome::Uncertain => "uncertain",
        EventOutcome::Waiting => "waiting",
        EventOutcome::HandedOff => "handed off",
    }
}

fn timeline_rows(
    events: &[crate::tasks::TaskTimelineEvent],
    dropped: usize,
    warning: Option<&str>,
    filter: TimelineFilter,
) -> Vec<String> {
    let mut lines = vec![if filter == TimelineFilter::All {
        "All events · recorded actions and outcomes".into()
    } else {
        format!("{} · recorded events", filter.label())
    }];
    if let Some(warning) = warning {
        lines.push(warning.into());
    }
    if dropped > 0 {
        lines.push(format!(
            "{dropped} older events omitted by the history limit."
        ));
    }
    let selected = events
        .iter()
        .filter(|event| filter.includes(event.kind))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        lines.push("No recorded events in this view.".into());
    }
    for event in selected {
        lines.push(format!(
            "{} · {}{} · {}",
            recorded_time(event.at_ms),
            event_label(event.kind),
            event
                .outcome
                .map(|outcome| format!(" ({})", outcome_label(outcome)))
                .unwrap_or_default(),
            event.subject
        ));
        lines.extend(event.detail.lines().map(|line| format!("  {line}")));
    }
    lines
}

fn timeline_lines(details: &TaskDetails, filter: TimelineFilter) -> Vec<String> {
    timeline_rows(
        &details.timeline,
        details.timeline_dropped,
        details.timeline_warning.as_deref(),
        filter,
    )
}

pub fn timeline_command(id: &str, json: bool) -> Result<u8> {
    let timeline = background::task_timeline(id)?;
    if json {
        crate::cli::json_contract::print_object(&timeline)?;
    } else {
        println!("Task timeline · {}", safe(id));
        for line in timeline_rows(
            &timeline.events,
            timeline.dropped,
            timeline.warning.as_deref(),
            TimelineFilter::All,
        ) {
            println!("{}", view_text(&line));
        }
    }
    Ok(0)
}

/// Queue a transfer for one exact native lease. This command does not start a
/// second execution owner or replay a task objective.
pub fn background_handoff(id: Option<&str>) -> Result<u8> {
    use crate::agent::native::handoff::{self, Direction};
    let snapshot = if let Some(id) = id {
        let native_id = match background::task_details(id) {
            Ok(details) => details
                .record
                .native_task_id
                .context("this task has no native checkpoint to hand off")?,
            Err(_) => crate::tasks::load(id)?.id,
        };
        handoff::request_task(&native_id, Direction::Background)?
    } else {
        let path = std::env::var_os("AISHE_NATIVE_HANDOFF_CONTROL").filter(|value| !value.is_empty()).map(PathBuf::from).context("no active foreground task is available; use /bg during a native task or specify its task ID")?;
        handoff::request_control(&path, Direction::Background)?
    };
    println!(
        "Handoff queued to background · {}. The task will checkpoint at its next safe boundary.",
        safe(&snapshot.task_id)
    );
    Ok(0)
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

/// Preserve every visible character in the bound action, including spaces
/// inside quoted commands and JSON strings. Prose word wrapping collapses them.
fn wrap_action_line(line: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut cells = 0;
    for grapheme in line.graphemes(true) {
        let next = crate::ui::cell_width(grapheme);
        if cells > 0 && cells + next > width.max(1) {
            rows.push(std::mem::take(&mut row));
            cells = 0;
        }
        row.push_str(grapheme);
        cells += next;
    }
    rows.push(row);
    rows
}

fn approval_review_lines(
    details: &TaskDetails,
    request: &background::InteractionRequest,
) -> Vec<String> {
    let mut lines = vec![
        details.entry.title.clone(),
        format!("Task: {}", details.record.id),
        format!("Request: {}", request.id),
        format!("Action: {}", request.binding.tool_name),
        format!(
            "Scope: {} · network {}",
            scope_label(request.binding.scope),
            network_label(request.binding.network)
        ),
        format!("Directory: {}", request.binding.cwd.display()),
        format!("Workspace: {}", request.binding.workspace_root.display()),
        "Only this exact action. The task keeps its scope and remaining budget.".into(),
        "Full action:".into(),
    ];
    lines.extend(request.prompt.lines().map(ToOwned::to_owned));
    lines.into_iter().map(|line| safe(&line)).collect()
}

fn approval_review_frame(
    content: &[String],
    scroll: usize,
    size: (usize, usize),
    static_mode: bool,
) -> (Vec<String>, usize, usize) {
    let width = size.0.saturating_sub(1).max(1);
    let budget = size.1.saturating_sub(1).clamp(1, MAX_FRAME_ROWS);
    let indent = if width > 2 { "  " } else { "" };
    let header = usize::from(budget > 2);
    let body = content
        .iter()
        .flat_map(|line| wrap_action_line(line, width.saturating_sub(indent.len()).max(1)))
        .collect::<Vec<_>>();
    let help = if static_mode {
        ["n next | p prev | home/end", "d choices | b back"]
    } else {
        ["Up/Down scroll | Home/End", "Enter choices | Esc back"]
    };
    let footer = help
        .into_iter()
        .map(|line| crate::ui::truncate_cells(&format!("{indent}{line}"), width))
        .take(budget.saturating_sub(header + 1))
        .collect::<Vec<_>>();
    let available = budget.saturating_sub(header + footer.len()).max(1);
    let maximum = body.len().saturating_sub(available);
    let offset = scroll.min(maximum);
    let end = (offset + available).min(body.len());
    let mut lines = Vec::new();
    if header > 0 {
        lines.push(crate::ui::truncate_cells(
            &view_text(&format!(
                "{indent}Review exact action · {}-{end}/{}",
                offset + 1,
                body.len()
            )),
            width,
        ));
    }
    lines.extend(
        body.into_iter()
            .skip(offset)
            .take(available)
            .map(|line| format!("{indent}{line}")),
    );
    lines.extend(footer);
    // Body rows were already sanitized and wrapped without changing quoted
    // action text. Do not pass them through prose or glyph substitutions.
    (lines.into_iter().take(budget).collect(), maximum, available)
}

fn review_exact_action(
    details: &TaskDetails,
    request: &background::InteractionRequest,
) -> Result<bool> {
    let content = approval_review_lines(details, request);
    let capabilities = TerminalCapabilities::detect_stdout();
    let mut scroll = 0_usize;
    if capabilities.motion == Motion::Static {
        loop {
            let (lines, maximum, available) =
                approval_review_frame(&content, scroll, promptui::terminal_size(), true);
            scroll = scroll.min(maximum);
            for line in lines {
                println!("{line}");
            }
            let Some(line) = promptui::read_terminal_line(true)? else {
                return Ok(false);
            };
            match line.trim() {
                "n" | ":next" => scroll = scroll.saturating_add(available).min(maximum),
                "p" | ":prev" => scroll = scroll.saturating_sub(available),
                "home" => scroll = 0,
                "end" => scroll = maximum,
                "d" => return Ok(true),
                "b" | ":back" | ":cancel" => return Ok(false),
                _ => {}
            }
        }
    }
    let mut input = PickerInput::open()?;
    let _raw = RawGuard::enter()?;
    let mut frame = Frame {
        capabilities,
        rows: 0,
        last: Vec::new(),
        size: (0, 0),
    };
    loop {
        let size = promptui::terminal_size();
        let (lines, maximum, available) = approval_review_frame(&content, scroll, size, false);
        scroll = scroll.min(maximum);
        frame.draw(lines, size);
        let Some(key) = input.read_live_key(150)? else {
            continue;
        };
        match key {
            PickerKey::Enter => return Ok(true),
            PickerKey::Cancel | PickerKey::Interrupt => return Ok(false),
            PickerKey::Up => scroll = scroll.saturating_sub(1),
            PickerKey::Down => scroll = scroll.saturating_add(1).min(maximum),
            PickerKey::PageUp => scroll = scroll.saturating_sub(available),
            PickerKey::PageDown => scroll = scroll.saturating_add(available).min(maximum),
            PickerKey::Home => scroll = 0,
            PickerKey::End => scroll = maximum,
            _ => {}
        }
    }
}

fn respond(config: &Config, details: &TaskDetails) -> Result<()> {
    let Some(request) = details.interaction.pending.first() else {
        return Ok(());
    };
    let id = details.record.id.clone();
    let request_id = request.id.clone();
    let action = match request.kind {
        InteractionKind::Question => {
            promptui::section("Needs you");
            for line in request.prompt.lines() {
                promptui::note(&safe(line));
            }
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
            let labels = [
                "Approve this exact action",
                "Deny and continue",
                "Review exact action",
                "Leave for later",
            ]
            .map(String::from);
            loop {
                if !review_exact_action(details, request)? {
                    return Ok(());
                }
                match promptui::filter_picker("Specific action approval", &labels, 3)? {
                    promptui::PickerResult::Use(0) => break Action::Approve { id, request_id },
                    promptui::PickerResult::Use(1) => {
                        let Some(reason) = read_message("Reason (optional)", "Denied by user")?
                        else {
                            return Ok(());
                        };
                        break Action::Deny {
                            id,
                            request_id,
                            reason,
                        };
                    }
                    promptui::PickerResult::Use(2) => continue,
                    _ => return Ok(()),
                }
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
        Control::Foreground => {
            let question = if matches!(record.state, State::Starting | State::Running) {
                "Bring this task into the foreground? It will pause at a safe boundary, then continue in this terminal with its saved scope and remaining budget."
            } else {
                "Continue this checkpoint in the foreground using its saved scope, model, and remaining budget?"
            };
            if promptui::confirm(question, false)? == Some(true) {
                crate::cli::runtime::resume_foreground_task(config, &id)?;
            }
            return Ok(());
        }
        Control::Apply => return crate::cli::changeui::review_and_apply(&id),
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
        Control::Reviewed => {
            background::mark_task_reviewed(&details.entry)?;
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
        Control::Views | Control::Actions | Control::TimelineFilter => return Ok(()),
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
        let entries = browser.entries.iter().collect::<Vec<_>>();
        for (id, line) in task_rows(&entries, None) {
            println!(
                "{}{}",
                id.map(|id| format!("{id} ")).unwrap_or_default(),
                view_text(&line)
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
                        entry.stage_name.as_deref().unwrap_or(&entry.title)
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
                "Task timeline".into(),
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
                    for line in change_lines(&record.id)? {
                        promptui::note(&safe(&line));
                    }
                }
                promptui::PickerResult::Use(2) => {
                    promptui::section("Recorded checks");
                    for line in evidence_lines(details) {
                        promptui::note(&safe(&line));
                    }
                }
                promptui::PickerResult::Use(3) => {
                    promptui::section("Task timeline");
                    for line in timeline_lines(details, TimelineFilter::All) {
                        promptui::note(&view_text(&line));
                    }
                }
                promptui::PickerResult::Use(index) if index < controls.len() + 4 => {
                    run_control(config, details, controls[index - 4].2)?;
                    let id = record.id.clone();
                    browser.refresh(project, true)?;
                    browser.open(&id, seen)?;
                }
                _ => page = Page::List,
            }
        }
    }
}

#[cfg(test)]
mod approval_review_tests {
    use super::{approval_review_frame, safe, wrap_action_line};

    #[test]
    fn exact_action_wrapping_preserves_quoted_spaces_and_unicode() {
        let action = safe(r#"run_command {"command":"printf '%s\\n' 'a   b  c · 界'"}"#);
        for width in [29, 55, 97] {
            let rows = wrap_action_line(&action, width);
            assert_eq!(rows.concat(), action);
            assert!(rows.iter().all(|row| crate::ui::cell_width(row) <= width));
        }
    }

    #[test]
    fn narrow_action_review_bounds_the_frame_and_keeps_the_entire_tail_reachable() {
        let content = vec![
            "Task: original-task; request: original-request".into(),
            format!("{} EXACT_ACTION_TAIL", "unchanged   action ".repeat(80)),
        ];
        for static_mode in [false, true] {
            let (first, maximum, _) = approval_review_frame(&content, 0, (32, 18), static_mode);
            let (last, _, _) = approval_review_frame(&content, maximum, (32, 18), static_mode);
            assert!(maximum > 0);
            for frame in [&first, &last] {
                assert!(frame.len() <= 17);
                assert!(frame.iter().all(|line| crate::ui::cell_width(line) <= 31));
                assert!(frame.last().unwrap().contains("back"));
            }
            let visible = last
                .iter()
                .map(|line| line.strip_prefix("  ").unwrap_or(line))
                .collect::<String>();
            assert!(visible.contains("EXACT_ACTION_TAIL"));
        }
    }
}
