use super::output::OutputParser;
use crate::clipboard;
use crate::logs;
use crate::runner::{JobResult, JobStatus};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const TAIL_BYTES: u64 = 64 * 1024;
const GREEN: (u8, u8, u8) = (158, 206, 106);
const RED: (u8, u8, u8) = (247, 118, 142);
const MUTED: (u8, u8, u8) = (143, 151, 199);

pub struct SummaryOptions {
    pub home: PathBuf,
    pub elapsed: Duration,
    pub tail_lines: usize,
    pub copy_failure_report: bool,
    /// Styles output with truecolor and OSC 8 links; callers enable it only for the interactive dashboard path.
    pub color: bool,
}

struct Failure<'a> {
    result: &'a JobResult,
    tail: Result<Vec<String>, String>,
}

/// Prints the end-of-run checklist and failure details, writes `summary.md` into the run directory, and copies the
/// Markdown report to the clipboard when a job failed and copying is enabled.
pub fn print_summary(results: &[JobResult], log_dir: Option<&Path>, options: &SummaryOptions) {
    let failures: Vec<Failure<'_>> = results
        .iter()
        .filter(|result| result.status == JobStatus::Failed)
        .map(|result| Failure {
            result,
            tail: match &result.log_path {
                Some(path) if options.tail_lines > 0 => read_tail(path, options.tail_lines),
                _ => Ok(Vec::new()),
            },
        })
        .collect();
    let markdown = render_markdown(results, &failures, log_dir, options.elapsed);
    let summary_path = log_dir.and_then(|directory| {
        match logs::create_job_log(directory, logs::SUMMARY_FILE)
            .and_then(|(path, mut file)| file.write_all(markdown.as_bytes()).map(|()| path))
        {
            Ok(path) => Some(path),
            Err(error) => {
                eprintln!("sup: cannot write run summary: {error}");
                None
            }
        }
    });

    let paint = Painter { color: options.color };
    print_checklist(results, log_dir, summary_path.as_deref(), options, paint);
    for failure in &failures {
        let result = failure.result;
        let exit = result
            .exit_code
            .map(|code| format!(" · exit {code}"))
            .unwrap_or_default();
        eprintln!();
        eprintln!(
            " {} {} ({}){exit} · {}",
            paint.apply(RED, "✗"),
            result.job.label,
            result.job.name,
            format_duration(result.elapsed)
        );
        if !result.reason.is_empty() {
            eprintln!("   reason  {}", result.reason);
        }

        if let Some(path) = &result.log_path {
            eprintln!("   log     {}", tilde(path, &options.home));
        }

        match &failure.tail {
            Ok(lines) => {
                for line in lines {
                    eprintln!("   {} {line}", paint.apply(MUTED, "│"));
                }
            }
            Err(error) => eprintln!("   {error}"),
        }
    }

    if !failures.is_empty() && options.copy_failure_report {
        eprintln!();
        match clipboard::copy(&markdown) {
            Ok(method) => eprintln!(" 📋 failure report copied via {method}"),
            Err(error) => eprintln!(" sup: cannot copy failure report: {error}"),
        }
    }
}

/// Applies truecolor styling only when the summary targets an interactive terminal.
#[derive(Clone, Copy)]
struct Painter {
    color: bool,
}

impl Painter {
    fn apply(self, rgb: (u8, u8, u8), text: &str) -> String {
        if self.color {
            format!("\x1b[38;2;{};{};{}m{text}\x1b[0m", rgb.0, rgb.1, rgb.2)
        } else {
            text.to_owned()
        }
    }
}

fn print_checklist(
    results: &[JobResult],
    log_dir: Option<&Path>,
    summary_path: Option<&Path>,
    options: &SummaryOptions,
    paint: Painter,
) {
    let width = results.iter().map(|result| result.job.name.len()).max().unwrap_or(0);
    for result in results {
        let (icon, rgb) = status_mark(&result.status);
        let time = paint.apply(MUTED, &format!("{:>7}", format_duration(result.elapsed)));
        let note = status_note(result);
        if note.is_empty() {
            println!(" {} {:<width$}  {time}", paint.apply(rgb, icon), result.job.name);
        } else {
            println!(
                " {} {:<width$}  {time}  {}",
                paint.apply(rgb, icon),
                result.job.name,
                paint.apply(MUTED, &note)
            );
        }
    }

    println!(" {}", paint.apply(MUTED, &"─".repeat(width + 18)));
    println!(" {}", count_line(results, options.elapsed));
    if let Some(directory) = log_dir {
        println!(
            " {}  {}",
            paint.apply(MUTED, "logs   "),
            tilde(directory, &options.home)
        );
    }

    if let Some(path) = summary_path {
        let shown = tilde(path, &options.home);
        if options.color {
            println!(
                " {}  \x1b]8;;file://{}\x1b\\{shown}\x1b]8;;\x1b\\",
                paint.apply(MUTED, "summary"),
                path.display()
            );
        } else {
            println!(" {}  {shown}", paint.apply(MUTED, "summary"));
        }
    }
}

fn render_markdown(
    results: &[JobResult],
    failures: &[Failure<'_>],
    log_dir: Option<&Path>,
    elapsed: Duration,
) -> String {
    let mut text = format!("# sup run: {}\n\n", count_line(results, elapsed));
    text.push_str("| | Job | Time | Note |\n| --- | --- | ---: | --- |\n");
    for result in results {
        text.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            status_mark(&result.status).0,
            result.job.name,
            format_duration(result.elapsed),
            status_note(result).replace('|', "\\|")
        ));
    }

    if let Some(directory) = log_dir {
        text.push_str(&format!("\nLogs: `{}`\n", directory.display()));
    }

    for failure in failures {
        let result = failure.result;
        text.push_str(&format!("\n## ✗ {} (`{}`)\n\n", result.job.label, result.job.name));
        if let Some(code) = result.exit_code {
            text.push_str(&format!("- Exit code: {code}\n"));
        }

        if !result.reason.is_empty() {
            text.push_str(&format!("- Reason: {}\n", result.reason));
        }

        if let Some(path) = &result.log_path {
            text.push_str(&format!("- Log: `{}`\n", path.display()));
        }

        match &failure.tail {
            Ok(lines) if !lines.is_empty() => text.push_str(&format!("\n````text\n{}\n````\n", lines.join("\n"))),
            Ok(_) => {}
            Err(error) => text.push_str(&format!("- {error}\n")),
        }
    }

    text
}

fn status_mark(status: &JobStatus) -> (&'static str, (u8, u8, u8)) {
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

fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(relative) if !home.as_os_str().is_empty() => format!("~/{}", relative.display()),
        _ => path.display().to_string(),
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
