use crate::auth::{AuthRequest, Password, PasswordReply, PlainPrompt};
use crate::config::Job;
use crate::runner::{Cancellation, JobResult, RunEvent};
use crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseEventKind};
use crossterm::execute;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};
use std::collections::{HashMap, VecDeque};
use std::io::{self, IsTerminal, Read, Seek, SeekFrom};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use zeroize::Zeroize;

const HISTORY_EVENTS: usize = 10_000;
const HISTORY_BYTES: usize = 8 * 1024 * 1024;
const EVENT_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputRecord {
    pub job: String,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Dashboard {
    records: VecDeque<OutputRecord>,
    bytes: usize,
    pending: HashMap<String, usize>,
    filters: HashMap<String, OutputFilter>,
    anchor: Option<usize>,
    following: bool,
}

impl Dashboard {
    pub fn push(&mut self, job: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        let text = self.filters.entry(job.to_owned()).or_default().clean(text);
        if text.is_empty() {
            return;
        }

        let pending_index = self.pending.remove(job);
        let mut combined = pending_index
            .and_then(|index| self.records.get(index))
            .map(|record| record.text.clone())
            .unwrap_or_default();
        combined.push_str(&text);
        let safe = truncate_event(normalize_lines(&combined));
        let mut lines: Vec<&str> = safe.split_terminator('\n').collect();
        let ends_line = safe.ends_with('\n');
        let mut unfinished = None;
        if !safe.ends_with('\n')
            && let Some(line) = lines.pop()
        {
            unfinished = Some(line);
        }

        if let Some(index) = pending_index
            && let Some(record) = self.records.get_mut(index)
        {
            if let Some(line) = lines.first() {
                self.bytes = self.bytes.saturating_sub(record.text.len());
                record.text = (*line).to_owned();
                self.bytes += record.text.len();
                lines.remove(0);
            } else if ends_line {
                self.bytes = self.bytes.saturating_sub(record.text.len());
                record.text.clear();
            } else if let Some(line) = unfinished.take() {
                self.bytes = self.bytes.saturating_sub(record.text.len());
                record.text = line.to_owned();
                self.bytes += record.text.len();
                self.pending.insert(job.to_owned(), index);
                self.trim_history();
                return;
            }
            self.trim_history();
        }
        self.push_lines(job, lines);
        if let Some(line) = unfinished {
            let index = self.append_record(job, line);
            self.pending.insert(job.to_owned(), index);
        }
    }

    pub fn finish_job(&mut self, job: &str) {
        self.pending.remove(job);
        self.filters.remove(job);
    }

    fn push_lines(&mut self, job: &str, lines: Vec<&str>) {
        for text in lines {
            self.append_record(job, text);
        }
    }

    fn append_record(&mut self, job: &str, text: &str) -> usize {
        self.bytes += text.len() + job.len();
        self.records.push_back(OutputRecord {
            job: job.to_owned(),
            text: text.to_owned(),
        });
        self.trim_history();
        self.records.len().saturating_sub(1)
    }

    fn trim_history(&mut self) {
        while self.records.len() > HISTORY_EVENTS || self.bytes > HISTORY_BYTES {
            if let Some(record) = self.records.pop_front() {
                self.bytes = self
                    .bytes
                    .saturating_sub(record.text.len() + record.job.len());
                self.pending.retain(|_, index| *index > 0);
                for index in self.pending.values_mut() {
                    *index -= 1;
                }
                if let Some(anchor) = self.anchor.as_mut() {
                    *anchor = anchor.saturating_sub(1);
                }
            } else {
                break;
            }
        }
    }

    pub fn scroll(&mut self, delta: isize) {
        if self.records.is_empty() {
            return;
        }
        let current = self.anchor.unwrap_or(self.records.len().saturating_sub(1)) as isize;
        self.anchor = Some((current + delta).clamp(0, self.records.len() as isize - 1) as usize);
        self.following = false;
    }

    pub fn resume(&mut self) {
        self.following = true;
        self.anchor = None;
    }

    pub fn records(&self) -> &VecDeque<OutputRecord> {
        &self.records
    }

    pub fn is_following(&self) -> bool {
        self.following
    }
}

impl Default for Dashboard {
    fn default() -> Self {
        Self {
            records: VecDeque::new(),
            bytes: 0,
            pending: HashMap::new(),
            filters: HashMap::new(),
            anchor: None,
            following: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
enum EscapeState {
    #[default]
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

#[derive(Clone, Debug, Default)]
struct OutputFilter {
    escape: EscapeState,
    carriage_return: bool,
}

impl OutputFilter {
    fn clean(&mut self, input: &str) -> String {
        let mut output = String::with_capacity(input.len());
        for character in input.chars() {
            match self.escape {
                EscapeState::Escape => {
                    self.escape = match character {
                        '[' => EscapeState::Csi,
                        ']' | 'P' | '^' | '_' => EscapeState::Osc,
                        _ => EscapeState::Text,
                    };
                    continue;
                }
                EscapeState::Csi => {
                    if ('@'..='~').contains(&character) {
                        self.escape = EscapeState::Text;
                    }

                    continue;
                }
                EscapeState::Osc | EscapeState::OscEscape => {
                    self.escape = match character {
                        '\x07' => EscapeState::Text,
                        '\\' if matches!(self.escape, EscapeState::OscEscape) => EscapeState::Text,
                        '\x1b' => EscapeState::OscEscape,
                        _ => EscapeState::Osc,
                    };
                    continue;
                }
                EscapeState::Text => {}
            }

            if character == '\x1b' {
                self.escape = EscapeState::Escape;
                continue;
            }

            if self.carriage_return {
                self.carriage_return = false;
                if character != '\n' {
                    output.push('\r');
                }
            }

            match character {
                '\r' => self.carriage_return = true,
                '\n' => output.push('\n'),
                '\t' => output.push(' '),
                character if !character.is_control() => output.push(character),
                _ => {}
            }
        }

        output
    }
}

fn normalize_lines(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut carriage = String::new();
    for character in input.chars() {
        if character == '\r' {
            carriage.clear();
            continue;
        }
        if character == '\n' {
            output.push_str(&carriage);
            output.push('\n');
            carriage.clear();
            continue;
        }
        if !character.is_control() || character == '\t' {
            if character == '\t' {
                carriage.push(' ');
            } else {
                carriage.push(character);
            }
        }
    }
    output.push_str(&carriage);
    output
}

fn truncate_event(mut text: String) -> String {
    if text.len() <= EVENT_BYTES {
        return text;
    }
    let marker = " … [event truncated]";
    let mut end = EVENT_BYTES - marker.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(marker);
    text
}

pub fn render(
    frame: &mut ratatui::Frame<'_>,
    dashboard: &Dashboard,
    jobs: &[Job],
    statuses: &HashMap<String, String>,
) {
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
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);
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
                statuses
                    .values()
                    .filter(|status| status.as_str() == "running")
                    .count(),
                statuses
                    .values()
                    .filter(|status| status.as_str() == "failed")
                    .count(),
                dashboard.records.len()
            ),
            Style::default().fg(tokyo_blue()),
        ),
    ]));
    frame.render_widget(title, vertical[0]);
    let height = vertical[1].height;
    let job_count = if height >= 12 {
        jobs.len().min(6).min(height.saturating_sub(11) as usize)
    } else {
        0
    };
    let job_rows = if job_count > 0 {
        job_count as u16 + 3
    } else {
        0
    };
    let hidden = jobs.len().saturating_sub(job_count);
    let middle = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(job_rows), Constraint::Min(1)])
        .split(vertical[1]);
    if job_rows > 0 {
        let mut ordered: Vec<&Job> = jobs.iter().collect();
        ordered.sort_by_key(|job| match statuses.get(&job.name).map(String::as_str) {
            Some("running") => 0,
            Some("failed") => 1,
            _ => 2,
        });
        let rows = ordered.into_iter().take(job_count).map(|job| {
            let status = statuses
                .get(&job.name)
                .cloned()
                .unwrap_or_else(|| "queued".to_owned());
            let color = match status.as_str() {
                "running" => tokyo_cyan(),
                "failed" => tokyo_red(),
                "succeeded" => tokyo_green(),
                _ => tokyo_muted(),
            };
            Row::new(vec![job.label.clone(), job.name.clone(), status])
                .style(Style::default().fg(color))
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
            Row::new(vec!["Job", "Name", "State"]).style(
                Style::default()
                    .fg(tokyo_blue())
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(tokyo_panel())),
        );
        frame.render_widget(table, middle[0]);
    }
    let hidden_text = if hidden > 0 {
        format!("  {hidden} jobs hidden")
    } else {
        String::new()
    };
    let heading = format!(" Output{hidden_text} ");
    let output = Block::default()
        .title(heading)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(tokyo_panel()));
    let inner = output.inner(middle[1]);
    frame.render_widget(output, middle[1]);
    if inner.width > 0 && inner.height > 0 {
        let visible = inner.height as usize;
        let last = dashboard
            .anchor
            .unwrap_or_else(|| dashboard.records.len().saturating_sub(1));
        let start = last.saturating_add(1).saturating_sub(visible);
        let mut lines: Vec<Line<'_>> = dashboard
            .records
            .iter()
            .skip(start)
            .take(last.saturating_sub(start) + 1)
            .map(|record| {
                let text = ratatui::text::Text::from(record.text.as_str())
                    .lines
                    .into_iter()
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>()
                    .join(" ⏎ ");
                Line::from(vec![
                    Span::styled(
                        format!("{} › ", record.job),
                        Style::default().fg(tokyo_purple()),
                    ),
                    Span::raw(text),
                ])
            })
            .collect();
        let padding = visible.saturating_sub(lines.len());
        let mut padded = vec![Line::default(); padding];
        padded.append(&mut lines);
        let paragraph = Paragraph::new(padded).alignment(Alignment::Left);
        frame.render_widget(paragraph, inner);
    }
    let hint = if dashboard.following {
        " ↑↓/PgUp PgDn/wheel scroll   End follow   Ctrl-C cancel "
    } else {
        " [PAUSED] ↑↓/PgUp PgDn/wheel scroll   End follow   Ctrl-C cancel "
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(tokyo_muted())),
        vertical[2],
    );
}

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
    let mut dashboard = Dashboard::default();
    let mut states = HashMap::new();
    let mut log_dir = None;
    let mut password: Option<(AuthRequest, Password)> = None;
    let mut disconnected = false;
    let mut last_draw = Instant::now();
    loop {
        for _ in 0..256 {
            match event_rx.try_recv() {
                Ok(event) => match event {
                    RunEvent::RunDirectory(path) => log_dir = Some(path),
                    RunEvent::Started(name) => {
                        states.insert(name, "running".to_owned());
                    }
                    RunEvent::Output { job, text } => dashboard.push(&job, &text),
                    RunEvent::Finished(result) => {
                        dashboard.finish_job(&result.job.name);
                        states.insert(
                            result.job.name.clone(),
                            format!("{:?}", result.status).to_lowercase(),
                        );
                    }
                },
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            }
        }
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
                    render(frame, &dashboard, jobs, &states);
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
            match event::read().map_err(|error| error.to_string())? {
                Event::Key(key) => {
                    if let Some((request, mut secret)) = password.take() {
                        match key.code {
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                request.one_use_reply.respond(PasswordReply::Cancel);
                                cancellation.cancel();
                            }
                            KeyCode::Esc => request.one_use_reply.respond(PasswordReply::Cancel),
                            KeyCode::Enter => {
                                request.one_use_reply.respond(PasswordReply::Submit(secret))
                            }
                            KeyCode::Backspace => {
                                secret.backspace();
                                password = Some((request, secret));
                            }
                            KeyCode::Char(ch)
                                if !key
                                    .modifiers
                                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                            {
                                let mut buffer = [0; 4];
                                let encoded = ch.encode_utf8(&mut buffer);
                                for byte in encoded.as_bytes() {
                                    if secret.push(*byte).is_err() {
                                        request.one_use_reply.respond(PasswordReply::Cancel);
                                        cancellation.cancel();
                                        buffer.zeroize();
                                        return Err(
                                            "password exceeded the supported length".to_owned()
                                        );
                                    }
                                }
                                buffer.zeroize();
                                password = Some((request, secret));
                            }
                            _ => password = Some((request, secret)),
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                cancellation.cancel()
                            }
                            KeyCode::Up => dashboard.scroll(-1),
                            KeyCode::Down => dashboard.scroll(1),
                            KeyCode::PageUp => {
                                let page = crossterm::terminal::size()
                                    .map(|(_, height)| height.saturating_sub(13).max(6) as isize)
                                    .unwrap_or(8);
                                dashboard.scroll(-page);
                            }
                            KeyCode::PageDown => {
                                let page = crossterm::terminal::size()
                                    .map(|(_, height)| height.saturating_sub(13).max(6) as isize)
                                    .unwrap_or(8);
                                dashboard.scroll(page);
                            }
                            KeyCode::End => dashboard.resume(),
                            _ => {}
                        }
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => dashboard.scroll(-3),
                    MouseEventKind::ScrollDown => dashboard.scroll(3),
                    _ => {}
                },
                Event::Resize(_, _) | Event::FocusGained | Event::FocusLost | Event::Paste(_) => {}
            }
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
        .draw(|frame| render(frame, &dashboard, jobs, &states))
        .map_err(|error| error.to_string())?;
    drop(terminal);
    if let Some(request) = password.take() {
        request.0.one_use_reply.respond(PasswordReply::Cancel);
    }
    Ok(log_dir)
}

fn plain_run(
    event_rx: Receiver<RunEvent>,
    auth_rx: Receiver<AuthRequest>,
    cancellation: Cancellation,
) -> Result<Option<std::path::PathBuf>, String> {
    let mut prompt = PlainPrompt::new();
    let mut log_dir = None;
    let mut filters: HashMap<String, OutputFilter> = HashMap::new();
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
                print!("[{job}] {}", normalize_lines(&text));
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
        if !result.reason.is_empty() {
            eprintln!("{}: {}", result.job.name, result.reason);
        }
        if let Some(path) = &result.log_path {
            println!("  log: {}", path.display());
            if result.exit_code.is_some_and(|code| code != 0)
                && let Ok(mut file) = std::fs::File::open(path)
            {
                let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                let start = length.saturating_sub(64 * 1024);
                if file.seek(SeekFrom::Start(start)).is_ok() {
                    let mut bytes = Vec::new();
                    let _ = file.read_to_end(&mut bytes);
                    let tail = String::from_utf8_lossy(&bytes);
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
            }
        }
    }
    if let Some(path) = log_dir {
        println!("Run logs: {}", path.display());
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
            let _ = execute!(
                stdout,
                crossterm::event::DisableMouseCapture,
                crossterm::cursor::Show,
                crossterm::terminal::LeaveAlternateScreen
            );
            let _ = crossterm::terminal::disable_raw_mode();
            return Err(error.to_string());
        }
        let backend = CrosstermBackend::new(stdout);
        match Terminal::new(backend) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = execute!(
                    std::io::stdout(),
                    crossterm::event::DisableMouseCapture,
                    crossterm::cursor::Show,
                    crossterm::terminal::LeaveAlternateScreen
                );
                let _ = crossterm::terminal::disable_raw_mode();
                Err(error.to_string())
            }
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            self.terminal.backend_mut(),
            crossterm::event::DisableMouseCapture,
            crossterm::cursor::Show,
            crossterm::terminal::LeaveAlternateScreen
        );
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = self.terminal.show_cursor();
    }
}
