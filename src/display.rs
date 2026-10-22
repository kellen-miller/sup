mod output;

use crate::auth::{AuthRequest, Password, PasswordReply, PlainPrompt};
use crate::config::Job;
use crate::runner::{Cancellation, JobResult, JobStatus, RunEvent};
use crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseEventKind};
use crossterm::execute;
use output::OutputParser;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};
use std::collections::{HashMap, VecDeque};
use std::io::{self, IsTerminal, Read, Seek, SeekFrom, Write};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use zeroize::Zeroize;

const HISTORY_EVENTS: usize = 10_000;
const HISTORY_BYTES: usize = 8 * 1024 * 1024;
const EVENT_BYTES: usize = 16 * 1024;
// A full pipe read can complete up to three UTF-8 bytes retained from the previous read.
const PIPE_EVENT_BYTES: usize = EVENT_BYTES + 3;
const TRUNCATION_MARKER: &str = " … [event truncated]";

#[derive(Clone, Debug, Eq, PartialEq)]
struct OutputStyle {
    start: usize,
    style: Style,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutputRecord {
    pub text: String,
    styles: Vec<OutputStyle>,
    age: u64,
    truncated: bool,
}

impl OutputRecord {
    /// Storage charged to pane history, including text, styles and the record.
    pub fn storage_bytes(&self) -> usize {
        size_of::<Self>() + self.text.capacity() + self.styles.capacity() * size_of::<OutputStyle>()
    }

    fn push(&mut self, character: char, style: Style) {
        if self.styles.last().map(|run| run.style) != Some(style) {
            self.styles.push(OutputStyle {
                start: self.text.len(),
                style,
            });
        }

        self.text.push(character);
    }

    fn truncate(&mut self) {
        if self.truncated {
            return;
        }

        let mut end = self.text.len().min(EVENT_BYTES - TRUNCATION_MARKER.len());
        while !self.text.is_char_boundary(end) {
            end -= 1;
        }

        self.text.truncate(end);
        self.styles.retain(|run| run.start < end);
        for character in TRUNCATION_MARKER.chars() {
            self.push(character, Style::default());
        }

        self.truncated = true;
    }

    fn line(&self) -> Line<'_> {
        Line::from(
            self.styles
                .iter()
                .enumerate()
                .map(|(index, run)| {
                    let end = self.styles.get(index + 1).map_or(self.text.len(), |next| next.start);
                    Span::styled(&self.text[run.start..end], run.style)
                })
                .collect::<Vec<_>>(),
        )
    }
}

pub struct JobPane {
    records: VecDeque<OutputRecord>,
    bytes: usize,
    pending: Option<usize>,
    filter: OutputParser,
    anchor: Option<usize>,
    following: bool,
    height: usize,
}

impl Default for JobPane {
    fn default() -> Self {
        Self {
            records: VecDeque::new(),
            bytes: 0,
            pending: None,
            filter: OutputParser::default(),
            anchor: None,
            following: true,
            height: 1,
        }
    }
}

impl JobPane {
    fn push(&mut self, text: &str, age: &mut u64) {
        let first = self.pending.unwrap_or(self.records.len());
        let mut current = self.pending.take();
        if let Some(index) = current {
            self.bytes = self.bytes.saturating_sub(self.records[index].storage_bytes());
        }

        // Ordinary pipe chunks fit completely; oversized events reserve diagnostic space.
        let limit = if text.len() > PIPE_EVENT_BYTES {
            EVENT_BYTES - TRUNCATION_MARKER.len()
        } else {
            PIPE_EVENT_BYTES
        };
        let mut used = 0;
        let mut truncated = false;
        let mut ended_line = false;
        self.filter.parse(text, |character, style| {
            ended_line = character == '\n';
            if truncated {
                return;
            }

            if character == '\r' {
                if let Some(index) = current {
                    let record = &mut self.records[index];
                    record.text.clear();
                    record.styles.clear();
                    record.truncated = false;
                }

                return;
            }

            if used + character.len_utf8() > limit {
                truncated = true;
                return;
            }

            used += character.len_utf8();
            let index = *current.get_or_insert_with(|| {
                self.records.push_back(OutputRecord {
                    age: *age,
                    ..OutputRecord::default()
                });
                *age = age.saturating_add(1);
                self.records.len() - 1
            });
            if character == '\n' {
                current = None;
                return;
            }

            let record = &mut self.records[index];
            if record.truncated {
                return;
            }

            if record.text.len() + character.len_utf8() > EVENT_BYTES {
                record.truncate();
            } else {
                record.push(character, style);
            }
        });
        if truncated
            && let Some(index) = current
                .or_else(|| self.records.len().checked_sub(1))
                .filter(|index| *index >= first)
        {
            self.records[index].truncate();
            current = (!ended_line).then_some(index);
        }

        self.pending = current;
        for record in self.records.iter_mut().skip(first) {
            record.text.shrink_to_fit();
            record.styles.shrink_to_fit();
            self.bytes += record.storage_bytes();
        }
    }

    fn evict_oldest(&mut self) {
        if let Some(record) = self.records.pop_front() {
            self.bytes = self.bytes.saturating_sub(record.storage_bytes());
            self.pending = self.pending.and_then(|index| index.checked_sub(1));
            self.anchor = self.anchor.map(|index| index.saturating_sub(1));
        }
    }

    pub fn records(&self) -> &VecDeque<OutputRecord> {
        &self.records
    }

    pub fn is_following(&self) -> bool {
        self.following
    }
}

pub struct Dashboard {
    panes: HashMap<String, JobPane>,
    statuses: HashMap<String, String>,
    order: Vec<String>,
    focus: Option<String>,
    visible: Vec<String>,
    page: usize,
    capacity: usize,
    bytes: usize,
    records: usize,
    next_age: u64,
}

impl Dashboard {
    pub fn new(jobs: &[Job]) -> Self {
        Self {
            panes: HashMap::new(),
            statuses: HashMap::new(),
            order: jobs.iter().map(|job| job.name.clone()).collect(),
            focus: None,
            visible: Vec::new(),
            page: 0,
            capacity: 0,
            bytes: 0,
            records: 0,
            next_age: 0,
        }
    }

    pub fn apply_event(&mut self, event: RunEvent) {
        match event {
            RunEvent::Started(name) => {
                self.panes.entry(name.clone()).or_default();
                self.statuses.insert(name.clone(), "running".to_owned());
                if self.focus.is_none() {
                    self.focus = Some(name);
                }
            }
            RunEvent::Output { job, text } => {
                if let Some(pane) = self.panes.get_mut(&job) {
                    self.bytes = self.bytes.saturating_sub(pane.bytes);
                    self.records = self.records.saturating_sub(pane.records.len());
                    pane.push(&text, &mut self.next_age);
                    self.bytes += pane.bytes;
                    self.records += pane.records.len();
                    self.trim_history();
                }
            }
            RunEvent::Finished(result) => {
                let name = &result.job.name;
                if let Some(pane) = self.panes.remove(name) {
                    self.bytes = self.bytes.saturating_sub(pane.bytes);
                    self.records = self.records.saturating_sub(pane.records.len());
                }

                self.visible.retain(|job| job != name);
                self.statuses
                    .insert(name.clone(), format!("{:?}", result.status).to_lowercase());
                if self.focus.as_ref() == Some(name) {
                    let index = self.order.iter().position(|job| job == name).unwrap_or(0);
                    self.focus = self.order[index..]
                        .iter()
                        .find(|job| self.panes.contains_key(*job))
                        .or_else(|| {
                            self.order[..index]
                                .iter()
                                .rev()
                                .find(|job| self.panes.contains_key(*job))
                        })
                        .cloned();
                }
            }
            RunEvent::RunDirectory(_) => {}
        }
    }

    fn trim_history(&mut self) {
        while self.records > HISTORY_EVENTS || self.bytes > HISTORY_BYTES {
            let oldest = self
                .panes
                .iter()
                .filter_map(|(name, pane)| pane.records.front().map(|record| (name.clone(), record.age)))
                .min_by_key(|(_, age)| *age);
            let Some((name, _)) = oldest else { break };
            if let Some(pane) = self.panes.get_mut(&name) {
                self.bytes = self.bytes.saturating_sub(pane.bytes);
                pane.evict_oldest();
                self.bytes += pane.bytes;
                self.records -= 1;
            }
        }
    }

    pub fn pane(&self, name: &str) -> Option<&JobPane> {
        self.panes.get(name)
    }

    pub fn focused_job(&self) -> Option<&str> {
        self.focus.as_deref()
    }

    pub fn scroll(&mut self, delta: isize) {
        if let Some(pane) = self.focus.as_ref().and_then(|name| self.panes.get_mut(name)) {
            if pane.records.is_empty() {
                return;
            }

            let current = pane.anchor.unwrap_or(pane.records.len() - 1) as isize;
            pane.anchor = Some((current + delta).clamp(0, pane.records.len() as isize - 1) as usize);
            pane.following = false;
        }
    }

    pub fn resume(&mut self) {
        if let Some(pane) = self.focus.as_ref().and_then(|name| self.panes.get_mut(name)) {
            pane.following = true;
            pane.anchor = None;
        }
    }

    pub fn change_page(&mut self, delta: isize) {
        let active: Vec<&String> = self
            .order
            .iter()
            .filter(|name| self.panes.contains_key(*name))
            .collect();
        let capacity = self.capacity.max(1);
        let last = active.len().saturating_sub(1) / capacity;
        let page = (self.page as isize + delta).clamp(0, last as isize) as usize;
        if page == self.page {
            return;
        }

        self.page = page;
        self.focus = active.get(self.page * capacity).map(|name| (*name).clone());
        self.visible = active
            .into_iter()
            .skip(self.page * capacity)
            .take(capacity)
            .cloned()
            .collect();
    }

    pub fn cycle_focus(&mut self, backwards: bool) {
        if self.visible.is_empty() {
            return;
        }

        let index = self
            .visible
            .iter()
            .position(|name| Some(name) == self.focus.as_ref())
            .unwrap_or(0);
        let step = if backwards { self.visible.len() - 1 } else { 1 };
        self.focus = Some(self.visible[(index + step) % self.visible.len()].clone());
    }

    fn page_height(&self) -> isize {
        self.focus
            .as_ref()
            .and_then(|name| self.panes.get(name))
            .map_or(1, |pane| pane.height.max(1)) as isize
    }
}

pub fn render(frame: &mut ratatui::Frame<'_>, dashboard: &mut Dashboard, jobs: &[Job]) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }

    frame.render_widget(
        Block::default().style(Style::default().fg(tokyo_fg()).bg(tokyo_bg())),
        area,
    );
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
        .split(area);
    render_title(frame, vertical[0], dashboard, jobs, &dashboard.statuses);
    let job_count = jobs.len().min(6).min(vertical[1].height.saturating_sub(11) as usize);
    let job_rows = if job_count > 0 { job_count as u16 + 3 } else { 0 };
    let middle = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(job_rows), Constraint::Min(0)])
        .split(vertical[1]);
    render_jobs(frame, middle[0], jobs, &dashboard.statuses, job_count);
    render_panes(frame, middle[1], dashboard, jobs);
    render_hint(frame, vertical[2], dashboard);
}

fn render_title(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    dashboard: &Dashboard,
    jobs: &[Job],
    statuses: &HashMap<String, String>,
) {
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " SUP ",
            Style::default()
                .fg(tokyo_bg())
                .bg(tokyo_cyan())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                " {} jobs · {} running · {} failed · {} events ",
                jobs.len(),
                statuses.values().filter(|status| status.as_str() == "running").count(),
                statuses.values().filter(|status| status.as_str() == "failed").count(),
                dashboard.records
            ),
            Style::default().fg(tokyo_blue()),
        ),
    ]));
    frame.render_widget(title, area);
}

fn render_jobs(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    jobs: &[Job],
    statuses: &HashMap<String, String>,
    job_count: usize,
) {
    if job_count == 0 {
        return;
    }
    let mut ordered: Vec<&Job> = jobs.iter().collect();
    ordered.sort_by_key(|job| match statuses.get(&job.name).map(String::as_str) {
        Some("running") => 0,
        Some("failed") => 1,
        _ => 2,
    });
    let rows = ordered.into_iter().take(job_count).map(|job| {
        let status = statuses.get(&job.name).cloned().unwrap_or_else(|| "queued".to_owned());
        let color = match status.as_str() {
            "running" => tokyo_cyan(),
            "failed" => tokyo_red(),
            "succeeded" => tokyo_green(),
            _ => tokyo_muted(),
        };
        Row::new(vec![job.label.clone(), job.name.clone(), status]).style(Style::default().fg(color))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(42),
            Constraint::Percentage(36),
            Constraint::Percentage(22),
        ],
    )
    .header(
        Row::new(vec!["Job", "Name", "State"]).style(Style::default().fg(tokyo_blue()).add_modifier(Modifier::BOLD)),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(tokyo_panel())),
    );
    frame.render_widget(table, area);
}

fn render_panes(frame: &mut ratatui::Frame<'_>, area: Rect, dashboard: &mut Dashboard, jobs: &[Job]) {
    let active: Vec<&Job> = jobs
        .iter()
        .filter(|job| dashboard.panes.contains_key(&job.name))
        .collect();
    let (max_columns, max_rows) = if area.width < 36 || area.height < 8 {
        (1, 1)
    } else {
        ((area.width / 36) as usize, (area.height / 8) as usize)
    };
    dashboard.capacity = max_columns * max_rows;
    if active.is_empty() {
        dashboard.focus = None;
        dashboard.visible.clear();
        dashboard.page = 0;
        frame.render_widget(
            Paragraph::new(" Waiting for jobs · finished panes close automatically "),
            area,
        );
        return;
    }

    let focus_index = active
        .iter()
        .position(|job| Some(&job.name) == dashboard.focus.as_ref());
    let focus_index = focus_index.unwrap_or_else(|| (dashboard.page * dashboard.capacity).min(active.len() - 1));
    dashboard.focus = Some(active[focus_index].name.clone());
    dashboard.page = focus_index / dashboard.capacity;
    let visible: Vec<&Job> = active
        .into_iter()
        .skip(dashboard.page * dashboard.capacity)
        .take(dashboard.capacity)
        .collect();
    dashboard.visible = visible.iter().map(|job| job.name.clone()).collect();
    let columns = max_columns.min(visible.len());
    let rows = visible.len().div_ceil(columns);
    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Fill(1); rows])
        .split(area);
    for (row, row_jobs) in visible.chunks(columns).enumerate() {
        let cells = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Fill(1); row_jobs.len()])
            .split(row_areas[row]);
        for (column, job) in row_jobs.iter().enumerate() {
            if let Some(pane) = dashboard.panes.get_mut(&job.name) {
                render_pane(
                    frame,
                    cells[column],
                    pane,
                    job,
                    dashboard.focus.as_ref() == Some(&job.name),
                );
            }
        }
    }
}

fn render_pane(frame: &mut ratatui::Frame<'_>, area: Rect, pane: &mut JobPane, job: &Job, focused: bool) {
    let state = if pane.following { "live" } else { "paused" };
    let marker = if focused { "* " } else { "" };
    let block = Block::default()
        .title(Span::styled(
            format!(" {marker}{} ({}) · {state} ", job.label, job.name),
            Style::default().fg(tokyo_fg()),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { tokyo_cyan() } else { tokyo_panel() }));
    let inner = block.inner(area);
    pane.height = inner.height as usize;
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let last = pane.anchor.unwrap_or_else(|| pane.records.len().saturating_sub(1));
    let start = last.saturating_add(1).saturating_sub(pane.height);
    let lines: Vec<Line<'_>> = pane
        .records
        .iter()
        .skip(start)
        .take(last.saturating_sub(start) + 1)
        .map(OutputRecord::line)
        .collect();
    let mut padded = vec![Line::default(); pane.height.saturating_sub(lines.len())];
    padded.extend(lines);
    frame.render_widget(Paragraph::new(padded).alignment(Alignment::Left), inner);
}

fn render_hint(frame: &mut ratatui::Frame<'_>, area: Rect, dashboard: &Dashboard) {
    let pages = dashboard.panes.len().div_ceil(dashboard.capacity.max(1)).max(1);
    let hidden = dashboard.panes.len().saturating_sub(dashboard.visible.len());
    let hint = format!(
        " {}/{} · {hidden} hidden  [] pages  Tab focus  ↑↓/Pg scroll  End follow  ^C cancel ",
        dashboard.page + 1,
        pages
    );
    frame.render_widget(Paragraph::new(hint).style(Style::default().fg(tokyo_muted())), area);
}

/// Run the interactive dashboard or its plain terminal fallback until the runner finishes.
///
/// # Errors
///
/// Returns an error when terminal setup, rendering, or input fails.
pub fn run(
    jobs: &[Job],
    event_rx: Receiver<RunEvent>,
    auth_rx: Receiver<AuthRequest>,
    cancellation: Cancellation,
    force_plain: bool,
) -> Result<Option<std::path::PathBuf>, String> {
    if force_plain || !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return plain_run(event_rx, auth_rx, cancellation);
    }
    let mut terminal = TerminalGuard::enter()?;
    let mut dashboard = Dashboard::new(jobs);
    let mut log_dir = None;
    let mut password: Option<(AuthRequest, Password)> = None;
    let mut disconnected = false;
    let mut last_draw = Instant::now();
    loop {
        disconnected |= collect_run_events(&event_rx, &mut dashboard, &mut log_dir);
        if password.is_none()
            && let Ok(request) = auth_rx.try_recv()
            && request.is_active()
        {
            password = Some((request, Password::new()));
        }
        if password
            .as_ref()
            .is_some_and(|(request, _)| !request.is_active() || cancellation.is_cancelled())
            && let Some((request, _)) = password.take()
        {
            request.one_use_reply.respond(PasswordReply::Cancel);
        }
        if last_draw.elapsed() >= Duration::from_millis(33) {
            let prompt = password.as_ref().map(|(request, secret)| {
                (
                    request.registered_job.clone(),
                    request.sanitized_prompt.clone(),
                    secret.len(),
                )
            });
            terminal
                .terminal
                .draw(|frame| {
                    render(frame, &mut dashboard, jobs);
                    if let Some((job, text, length)) = &prompt {
                        render_password(frame, job, text, *length);
                    }
                })
                .map_err(|error| error.to_string())?;
            last_draw = Instant::now();
        }
        if disconnected {
            break;
        }
        if event::poll(Duration::from_millis(15)).map_err(|error| error.to_string())? {
            handle_terminal_event(
                event::read().map_err(|error| error.to_string())?,
                &mut password,
                &mut dashboard,
                &cancellation,
            )?;
        }
        if password.is_none()
            && let Ok(request) = auth_rx.try_recv()
            && request.is_active()
        {
            password = Some((request, Password::new()));
        }
    }
    terminal
        .terminal
        .draw(|frame| render(frame, &mut dashboard, jobs))
        .map_err(|error| error.to_string())?;
    drop(terminal);
    if let Some(request) = password.take() {
        request.0.one_use_reply.respond(PasswordReply::Cancel);
    }
    Ok(log_dir)
}

fn collect_run_events(
    event_rx: &Receiver<RunEvent>,
    dashboard: &mut Dashboard,
    log_dir: &mut Option<std::path::PathBuf>,
) -> bool {
    for _ in 0..256 {
        let event = match event_rx.try_recv() {
            Ok(event) => event,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return true,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
        };
        if let RunEvent::RunDirectory(path) = event {
            *log_dir = Some(path);
        } else {
            dashboard.apply_event(event);
        }
    }
    false
}

fn handle_terminal_event(
    input: Event,
    password: &mut Option<(AuthRequest, Password)>,
    dashboard: &mut Dashboard,
    cancellation: &Cancellation,
) -> Result<(), String> {
    match input {
        Event::Key(key) => match password.take() {
            Some((request, secret)) => handle_password_key(key, request, secret, password, cancellation)?,
            None => handle_dashboard_key(key, dashboard, cancellation),
        },
        Event::Mouse(mouse) if password.is_none() => match mouse.kind {
            MouseEventKind::ScrollUp => dashboard.scroll(-3),
            MouseEventKind::ScrollDown => dashboard.scroll(3),
            _ => {}
        },
        Event::Mouse(_) | Event::Resize(_, _) | Event::FocusGained | Event::FocusLost | Event::Paste(_) => {}
    }
    Ok(())
}

fn handle_password_key(
    key: crossterm::event::KeyEvent,
    request: AuthRequest,
    mut secret: Password,
    password: &mut Option<(AuthRequest, Password)>,
    cancellation: &Cancellation,
) -> Result<(), String> {
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            request.one_use_reply.respond(PasswordReply::Cancel);
            cancellation.cancel();
        }
        KeyCode::Esc => request.one_use_reply.respond(PasswordReply::Cancel),
        KeyCode::Enter => request.one_use_reply.respond(PasswordReply::Submit(secret)),
        KeyCode::Backspace => {
            secret.backspace();
            *password = Some((request, secret));
        }
        KeyCode::Char(ch) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            let mut buffer = [0; 4];
            let encoded = ch.encode_utf8(&mut buffer);
            let append_failed = encoded.as_bytes().iter().any(|byte| secret.push(*byte).is_err());
            buffer.zeroize();
            if append_failed {
                request.one_use_reply.respond(PasswordReply::Cancel);
                cancellation.cancel();
                return Err("password exceeded the supported length".to_owned());
            }
            *password = Some((request, secret));
        }
        _ => *password = Some((request, secret)),
    }
    Ok(())
}

fn handle_dashboard_key(key: crossterm::event::KeyEvent, dashboard: &mut Dashboard, cancellation: &Cancellation) {
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => cancellation.cancel(),
        KeyCode::Up => dashboard.scroll(-1),
        KeyCode::Down => dashboard.scroll(1),
        KeyCode::PageUp => dashboard.scroll(-dashboard.page_height()),
        KeyCode::PageDown => dashboard.scroll(dashboard.page_height()),
        KeyCode::End => dashboard.resume(),
        KeyCode::Tab => dashboard.cycle_focus(false),
        KeyCode::BackTab => dashboard.cycle_focus(true),
        KeyCode::Char('[') => dashboard.change_page(-1),
        KeyCode::Char(']') => dashboard.change_page(1),
        _ => {}
    }
}

fn plain_run(
    event_rx: Receiver<RunEvent>,
    auth_rx: Receiver<AuthRequest>,
    cancellation: Cancellation,
) -> Result<Option<std::path::PathBuf>, String> {
    let mut prompt = PlainPrompt::new();
    let mut log_dir = None;
    let mut filters: HashMap<String, OutputParser> = HashMap::new();
    loop {
        while let Ok(request) = auth_rx.try_recv() {
            if io::stdin().is_terminal() {
                prompt.respond(request, &cancellation);
            } else {
                request.one_use_reply.respond(PasswordReply::Cancel);
                eprintln!("sup: interactive password dialog unavailable without a terminal");
            }
        }
        match event_rx.recv_timeout(Duration::from_millis(20)) {
            Ok(RunEvent::RunDirectory(path)) => log_dir = Some(path),
            Ok(RunEvent::Started(name)) => println!("Running {name}"),
            Ok(RunEvent::Output { job, text }) => {
                let text = filters.entry(job.clone()).or_default().clean(&text);
                print!("[{job}] {}", text);
            }
            Ok(RunEvent::Finished(result)) => {
                filters.remove(&result.job.name);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    Ok(log_dir)
}

pub fn print_summary(results: &[JobResult], log_dir: Option<&std::path::Path>, tail_lines: usize) {
    for result in results {
        println!(
            "{}: {:?} ({:.1}s)",
            result.job.name,
            result.status,
            result.elapsed.as_secs_f64()
        );
        if result.status != JobStatus::Failed {
            if !result.reason.is_empty() {
                println!("  {}", result.reason);
            }

            if let Some(path) = &result.log_path {
                println!("  log: {}", path.display());
            }
        }
    }

    if results.iter().any(|result| result.status == JobStatus::Failed) {
        eprintln!("Failure report:");
        for result in results.iter().filter(|result| result.status == JobStatus::Failed) {
            eprintln!("{} ({})", result.job.label, result.job.name);
            if let Some(code) = result.exit_code {
                eprintln!("  exit code: {code}");
            }

            if !result.reason.is_empty() {
                eprintln!("  reason: {}", result.reason);
            }

            if let Some(path) = &result.log_path {
                eprintln!("  log: {}", path.display());
                if tail_lines > 0 {
                    print_log_tail(path, tail_lines);
                }
            }
        }
    }

    if let Some(path) = log_dir {
        println!("Run logs: {}", path.display());
    }
}

fn print_log_tail(path: &std::path::Path, tail_lines: usize) {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("  cannot read log {}: {error}", path.display());
            return;
        }
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    if let Err(error) = file.seek(SeekFrom::Start(length.saturating_sub(64 * 1024))) {
        eprintln!("  cannot seek log {}: {error}", path.display());
        return;
    }
    let mut bytes = Vec::new();
    if let Err(error) = file.take(64 * 1024).read_to_end(&mut bytes) {
        eprintln!("  cannot read log {}: {error}", path.display());
        return;
    }
    let tail = OutputParser::default().clean(&String::from_utf8_lossy(&bytes));
    for line in tail
        .lines()
        .rev()
        .take(tail_lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        eprintln!("  {line}");
    }
}

fn render_password(frame: &mut ratatui::Frame<'_>, job: &str, prompt: &str, length: usize) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    if area.width < 8 || area.height < 4 {
        frame.render_widget(
            Paragraph::new(format!("{job} password: {}", "•".repeat(length)))
                .style(Style::default().fg(tokyo_fg()).bg(tokyo_panel())),
            area,
        );
        return;
    }
    let width = area.width.min(62).saturating_sub(2);
    let height = 5.min(area.height);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::default()
        .title(" Authentication ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(tokyo_purple()));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let content = vec![
        Line::from(format!("{job} · {prompt}")),
        Line::from(format!("{}  Enter submit · Esc cancel", "•".repeat(length))),
    ];
    frame.render_widget(Paragraph::new(content), inner);
}

fn tokyo_bg() -> Color {
    Color::Rgb(26, 27, 38)
}

fn tokyo_panel() -> Color {
    Color::Rgb(36, 40, 59)
}

fn tokyo_blue() -> Color {
    Color::Rgb(122, 162, 247)
}

fn tokyo_cyan() -> Color {
    Color::Rgb(125, 207, 255)
}

fn tokyo_green() -> Color {
    Color::Rgb(158, 206, 106)
}

fn tokyo_red() -> Color {
    Color::Rgb(247, 118, 142)
}

fn tokyo_purple() -> Color {
    Color::Rgb(187, 154, 247)
}

fn tokyo_fg() -> Color {
    Color::Rgb(192, 202, 245)
}

fn tokyo_muted() -> Color {
    Color::Rgb(143, 151, 199)
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
}

impl TerminalGuard {
    fn enter() -> Result<Self, String> {
        crossterm::terminal::enable_raw_mode().map_err(|error| error.to_string())?;
        let mut stdout = std::io::stdout();
        if let Err(error) = execute!(
            stdout,
            crossterm::terminal::EnterAlternateScreen,
            crossterm::event::EnableMouseCapture,
            crossterm::cursor::Hide
        ) {
            if let Err(cleanup_error) = restore_screen(&mut stdout) {
                eprintln!("sup: terminal cleanup failed: {cleanup_error}");
            }
            if let Err(cleanup_error) = crossterm::terminal::disable_raw_mode() {
                eprintln!("sup: raw mode cleanup failed: {cleanup_error}");
            }
            return Err(error.to_string());
        }
        let backend = CrosstermBackend::new(stdout);
        match Terminal::new(backend) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let mut stdout = std::io::stdout();
                if let Err(cleanup_error) = restore_screen(&mut stdout) {
                    eprintln!("sup: terminal cleanup failed: {cleanup_error}");
                }
                if let Err(cleanup_error) = crossterm::terminal::disable_raw_mode() {
                    eprintln!("sup: raw mode cleanup failed: {cleanup_error}");
                }
                Err(error.to_string())
            }
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Err(error) = restore_screen(self.terminal.backend_mut()) {
            eprintln!("sup: terminal cleanup failed: {error}");
        }
        if let Err(error) = crossterm::terminal::disable_raw_mode() {
            eprintln!("sup: raw mode cleanup failed: {error}");
        }
        if let Err(error) = self.terminal.show_cursor() {
            eprintln!("sup: cursor cleanup failed: {error}");
        }
    }
}

fn restore_screen(output: &mut impl Write) -> io::Result<()> {
    let mut first_error = None;
    if let Err(error) = execute!(output, crossterm::event::DisableMouseCapture) {
        first_error = Some(error);
    }
    if let Err(error) = execute!(output, crossterm::cursor::Show)
        && first_error.is_none()
    {
        first_error = Some(error);
    }
    if let Err(error) = execute!(output, crossterm::terminal::LeaveAlternateScreen)
        && first_error.is_none()
    {
        first_error = Some(error);
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::print_summary;
    use crate::config::{AuthMode, Job};
    use crate::runner::{JobResult, JobStatus};
    use std::path::Path;
    use std::process::Command;
    use std::time::Duration;

    fn report_results(directory: &Path) -> Vec<JobResult> {
        [
            ("missing", JobStatus::Failed, None, None),
            ("zero", JobStatus::Failed, Some(0), Some(directory.join("output.log"))),
            (
                "unreadable",
                JobStatus::Failed,
                None,
                Some(directory.join("absent.log")),
            ),
            ("after", JobStatus::Failed, Some(4), Some(directory.join("output.log"))),
            ("skip", JobStatus::Skipped, None, None),
            ("cancel", JobStatus::Cancelled, None, None),
        ]
        .into_iter()
        .map(|(name, status, exit_code, log_path)| JobResult {
            job: Job {
                name: name.to_owned(),
                label: format!("Label {name}"),
                phase: "core".to_owned(),
                command: vec!["/bin/true".to_owned()],
                required_commands: Vec::new(),
                required_paths: Vec::new(),
                required_env: Vec::new(),
                optional: false,
                log_name: format!("{name}.log"),
                auth: AuthMode::None,
            },
            status,
            exit_code,
            elapsed: Duration::ZERO,
            log_path,
            reason: format!("because {name}"),
        })
        .collect()
    }

    #[test]
    fn failure_report_handles_missing_zero_codes_and_unreadable_logs() {
        // A child test process captures the actual public stdout/stderr contract.
        if let Some(directory) = std::env::var_os("SUP_REPORT_FIXTURE") {
            let tail = std::env::var("SUP_REPORT_TAIL").unwrap().parse().unwrap();
            print_summary(&report_results(Path::new(&directory)), None, tail);
            return;
        }

        let directory = tempfile::tempdir().unwrap();
        let raw = format!("{}\n\x1b[31mlast-one\x1b[0m\nlast-two\n", "early\n".repeat(20_000));
        std::fs::write(directory.path().join("output.log"), &raw).unwrap();
        for tail in [2, 0] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "display::tests::failure_report_handles_missing_zero_codes_and_unreadable_logs",
                    "--nocapture",
                ])
                .env("SUP_REPORT_FIXTURE", directory.path())
                .env("SUP_REPORT_TAIL", tail.to_string())
                .output()
                .unwrap();
            assert!(output.status.success());
            let report = String::from_utf8_lossy(&output.stderr);
            assert_eq!(report.matches("Failure report:").count(), 1);
            assert!(
                report.contains("Label missing (missing)\n  reason: because missing\n"),
                "{report}"
            );
            assert!(report.contains("Label zero (zero)\n  exit code: 0\n"), "{report}");
            assert!(report.contains("Label after (after)\n  exit code: 4\n"), "{report}");
            assert_eq!(report.contains("cannot read log"), tail == 2, "{report}");
            assert_eq!(
                report.matches("  last-one\n  last-two\n").count(),
                if tail == 2 { 2 } else { 0 }
            );
            assert!(!report.contains("early"));
            assert!(!report.contains('\x1b'));
            assert!(!report.contains("Label skip"));
            assert!(!report.contains("Label cancel"));
            assert_eq!(
                std::fs::read_to_string(directory.path().join("output.log")).unwrap(),
                raw
            );
        }
    }
}
