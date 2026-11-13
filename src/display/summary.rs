use super::output::OutputParser;
use crate::clipboard;
use crate::logs;
use crate::runner::{JobResult, JobStatus};
use askama::Template;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const TAIL_BYTES: u64 = 64 * 1024;
const GREEN: Rgb = (158, 206, 106);
const RED: Rgb = (247, 118, 142);
const MUTED: Rgb = (143, 151, 199);

pub struct SummaryOptions {
    pub home: PathBuf,
    pub elapsed: Duration,
    pub tail_lines: usize,
    pub copy_failure_report: bool,
    /// Styles output with truecolor and OSC 8 links; callers enable it only for the interactive dashboard path.
    pub color: bool,
}

type Rgb = (u8, u8, u8);

/// Display-ready summary data shared by the terminal and Markdown templates.
struct SummaryView {
    counts: String,
    name_width: usize,
    jobs: Vec<JobRow>,
    failures: Vec<FailureView>,
    log_dir: Option<PathView>,
}

impl SummaryView {
    fn rule(&self) -> String {
        "─".repeat(self.name_width + 18)
    }
}

struct JobRow {
    icon: &'static str,
    rgb: Rgb,
    name: String,
    time: String,
    note: String,
}

impl JobRow {
    fn markdown_note(&self) -> String {
        self.note.replace('|', "\\|")
    }
}

struct FailureView {
    label: String,
    name: String,
    exit_code: Option<i32>,
    time: String,
    reason: String,
    log: Option<PathView>,
    tail: Vec<String>,
    tail_error: Option<String>,
}

/// A path as written to Markdown (`full`) and shown in the terminal (`short`, home-relative).
struct PathView {
    full: String,
    short: String,
}

impl PathView {
    fn new(path: &Path, home: &Path) -> Self {
        let short = match path.strip_prefix(home) {
            Ok(relative) if !home.as_os_str().is_empty() => format!("~/{}", relative.display()),
            _ => path.display().to_string(),
        };
        Self {
            full: path.display().to_string(),
            short,
        }
    }
}

#[derive(Template)]
#[template(path = "summary.md", escape = "none")]
struct MarkdownTemplate<'a> {
    view: &'a SummaryView,
}

#[derive(Template)]
#[template(path = "checklist.txt", escape = "none")]
struct ChecklistTemplate<'a> {
    view: &'a SummaryView,
    summary: Option<PathView>,
    paint: Painter,
}

#[derive(Template)]
#[template(path = "failures.txt", escape = "none")]
struct FailuresTemplate<'a> {
    view: &'a SummaryView,
    paint: Painter,
}

/// Prints the end-of-run checklist and failure details, writes `summary.md` into the run directory, and copies the
/// Markdown report to the clipboard when a job failed and copying is enabled.
pub fn print_summary(results: &[JobResult], log_dir: Option<&Path>, options: &SummaryOptions) {
    let view = SummaryView {
        counts: count_line(results, options.elapsed),
        name_width: results.iter().map(|result| result.job.name.len()).max().unwrap_or(0),
        jobs: results
            .iter()
            .map(|result| {
                let (icon, rgb) = status_mark(&result.status);
                JobRow {
                    icon,
                    rgb,
                    name: result.job.name.clone(),
                    time: format_duration(result.elapsed),
                    note: status_note(result),
                }
            })
            .collect(),
        failures: results
            .iter()
            .filter(|result| result.status == JobStatus::Failed)
            .map(|result| {
                let tail = match &result.log_path {
                    Some(path) if options.tail_lines > 0 => read_tail(path, options.tail_lines),
                    _ => Ok(Vec::new()),
                };
                FailureView {
                    label: result.job.label.clone(),
                    name: result.job.name.clone(),
                    exit_code: result.exit_code,
                    time: format_duration(result.elapsed),
                    reason: result.reason.clone(),
                    log: result
                        .log_path
                        .as_deref()
                        .map(|path| PathView::new(path, &options.home)),
                    tail_error: tail.as_ref().err().cloned(),
                    tail: tail.unwrap_or_default(),
                }
            })
            .collect(),
        log_dir: log_dir.map(|path| PathView::new(path, &options.home)),
    };
    let paint = Painter { color: options.color };
    let rendered = MarkdownTemplate { view: &view }.render().and_then(|markdown| {
        let summary_path = log_dir.and_then(|directory| {
            match logs::create_job_log(directory, logs::SUMMARY_FILE)
                .and_then(|(path, mut file)| file.write_all(markdown.as_bytes()).map(|()| path))
            {
                Ok(path) => Some(PathView::new(&path, &options.home)),
                Err(error) => {
                    eprintln!("sup: cannot write run summary: {error}");
                    None
                }
            }
        });
        let checklist = ChecklistTemplate {
            view: &view,
            summary: summary_path,
            paint,
        }
        .render()?;
        let failures = FailuresTemplate { view: &view, paint }.render()?;
        Ok((markdown, checklist, failures))
    });
    let (markdown, checklist, failures) = match rendered {
        Ok(rendered) => rendered,
        Err(error) => {
            eprintln!("sup: cannot render run summary: {error}");
            return;
        }
    };

    print!("{checklist}");
    eprint!("{failures}");
    if !view.failures.is_empty() && options.copy_failure_report {
        eprintln!();
        match clipboard::copy(&markdown) {
            Ok(method) => eprintln!(" 📋 failure report copied via {method}"),
            Err(error) => eprintln!(" sup: cannot copy failure report: {error}"),
        }
    }
}

/// Applies truecolor styling and OSC 8 links only when the summary targets an interactive terminal.
#[derive(Clone, Copy)]
struct Painter {
    color: bool,
}

impl Painter {
    fn apply(self, rgb: &Rgb, text: impl std::fmt::Display) -> String {
        if self.color {
            format!("\x1b[38;2;{};{};{}m{text}\x1b[0m", rgb.0, rgb.1, rgb.2)
        } else {
            text.to_string()
        }
    }

    fn link(self, target: &str, text: &str) -> String {
        if self.color {
            format!("\x1b]8;;file://{target}\x1b\\{text}\x1b]8;;\x1b\\")
        } else {
            text.to_owned()
        }
    }
}

fn status_mark(status: &JobStatus) -> (&'static str, Rgb) {
    match status {
        JobStatus::Succeeded => ("✓", GREEN),
        JobStatus::Failed => ("✗", RED),
        JobStatus::Skipped => ("○", MUTED),
        JobStatus::Cancelled => ("⊘", MUTED),
    }
}

fn status_note(result: &JobResult) -> String {
    match (&result.status, result.exit_code) {
        (JobStatus::Succeeded, _) => String::new(),
        (JobStatus::Failed, Some(code)) => format!("exit {code}"),
        (JobStatus::Skipped, _) => format!("skipped: {}", result.reason),
        (JobStatus::Cancelled, _) => format!("cancelled: {}", result.reason),
        (JobStatus::Failed, None) => result.reason.lines().next().unwrap_or("failed").to_owned(),
    }
}

fn count_line(results: &[JobResult], elapsed: Duration) -> String {
    let count = |status: JobStatus| results.iter().filter(|result| result.status == status).count();
    let mut parts = vec![format!("{} passed", count(JobStatus::Succeeded))];
    for (status, word) in [
        (JobStatus::Failed, "failed"),
        (JobStatus::Skipped, "skipped"),
        (JobStatus::Cancelled, "cancelled"),
    ] {
        let total = count(status);
        if total > 0 {
            parts.push(format!("{total} {word}"));
        }
    }

    parts.push(format_duration(elapsed));
    parts.join(" · ")
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

fn read_tail(path: &Path, tail_lines: usize) -> Result<Vec<String>, String> {
    let cannot = |error: io::Error| format!("cannot read log {}: {error}", path.display());
    let mut file = std::fs::File::open(path).map_err(cannot)?;
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    file.seek(SeekFrom::Start(length.saturating_sub(TAIL_BYTES)))
        .map_err(cannot)?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes).map_err(cannot)?;
    let text = OutputParser::default().clean(&String::from_utf8_lossy(&bytes));
    let lines: Vec<&str> = text.lines().collect();
    Ok(lines[lines.len().saturating_sub(tail_lines)..]
        .iter()
        .map(|line| (*line).to_owned())
        .collect())
}
