#![cfg(test)]

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::collections::HashMap;
use std::path::PathBuf;
use sup::config::{AuthMode, Job};
use sup::display::Dashboard;

fn job(name: &str) -> Job {
    Job {
        name: name.to_owned(),
        label: name.to_owned(),
        phase: "core".to_owned(),
        command: vec!["true".to_owned()],
        required_commands: Vec::new(),
        required_paths: Vec::<PathBuf>::new(),
        required_env: Vec::new(),
        optional: false,
        log_name: format!("{name}.log"),
        auth: AuthMode::None,
    }
}

#[test]
fn tall_dashboard_shows_more_than_ten_recent_records_at_bottom() {
    let mut terminal = Terminal::new(TestBackend::new(120, 60)).unwrap();
    let mut dashboard = Dashboard::default();
    for index in 0..30 {
        dashboard.push("build", &format!("record-{index}\n"));
    }
    terminal
        .draw(|frame| sup::display::render(frame, &dashboard, &[job("build")], &HashMap::new()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..60)
        .map(|row| (0..120).map(|column| buffer[(column, row)].symbol()).collect())
        .collect();
    let shown: Vec<_> = rows.iter().filter(|row| row.contains("record-")).collect();
    assert!(shown.len() > 10, "visible output rows: {}", shown.len());
    assert!(rows.iter().rev().take(3).any(|row| row.contains("record-29")));
    assert!(rows[59].contains("End follow"));
}

#[test]
fn history_caps_each_event_and_preserves_follow_and_scroll_anchor() {
    let mut dashboard = Dashboard::default();
    dashboard.push("job", &format!("{}\n", "x".repeat(20 * 1024)));
    dashboard.finish_job("job");
    assert!(dashboard.records().back().unwrap().text.contains("event truncated"));
    for index in 0..20 {
        dashboard.push("job", &format!("line-{index}\n"));
    }
    dashboard.scroll(-3);
    let anchor = dashboard.records()[dashboard.records().len() - 4].text.clone();
    dashboard.push("job", "new output");
    dashboard.finish_job("job");
    assert!(!dashboard.is_following());
    assert_eq!(dashboard.records()[dashboard.records().len() - 5].text, anchor);
    dashboard.resume();
    assert!(dashboard.is_following());
}

#[test]
fn output_chunks_keep_partial_lines_and_drop_only_terminating_empty_row() {
    let mut dashboard = Dashboard::default();
    dashboard.push("job", "first\nsecond\n");
    dashboard.push("job", "part");
    dashboard.push("job", "ial\n");
    assert_eq!(dashboard.records().len(), 3);
    assert_eq!(dashboard.records()[0].text, "first");
    assert_eq!(dashboard.records()[1].text, "second");
    assert_eq!(dashboard.records()[2].text, "partial");
}

#[test]
fn partial_progress_is_visible_before_the_job_finishes() {
    let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();
    let mut dashboard = Dashboard::default();
    dashboard.push("package", "progress 20%");
    terminal
        .draw(|frame| sup::display::render(frame, &dashboard, &[job("package")], &HashMap::new()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let screen = (0..18)
        .map(|row| (0..80).map(|column| buffer[(column, row)].symbol()).collect::<String>())
        .collect::<String>();
    assert!(screen.contains("progress 20%"));
}

#[test]
fn carriage_return_replaces_cross_chunk_progress_without_ansi_controls() {
    let mut dashboard = Dashboard::default();
    dashboard.push("build", "20% complete");
    dashboard.push("build", "\r\u{1b}[31m40% complete\u{1b}[0m");
    assert_eq!(dashboard.records().len(), 1);
    assert_eq!(dashboard.records()[0].text, "40% complete");
}

#[test]
fn tiny_dimensions_render_without_panicking() {
    for (width, height) in [(1, 1), (4, 3), (12, 8), (80, 18), (120, 24)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| sup::display::render(frame, &Dashboard::default(), &[job("build")], &HashMap::new()))
            .unwrap();
    }
}

#[test]
fn eighteen_row_dashboard_reserves_six_output_rows_and_footer() {
    let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();
    let mut dashboard = Dashboard::default();
    for index in 0..12 {
        dashboard.push("build", &format!("row-{index}\n"));
    }
    terminal
        .draw(|frame| sup::display::render(frame, &dashboard, &[job("build")], &HashMap::new()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let visible = (0..18)
        .filter(|row| {
            (0..80)
                .map(|column| buffer[(column, *row)].symbol())
                .collect::<String>()
                .contains("row-")
        })
        .count();
    assert!(visible >= 6, "only {visible} output rows were visible");
    assert!(
        (0..80)
            .map(|column| buffer[(column, 17)].symbol())
            .collect::<String>()
            .contains("End follow")
    );
}

#[test]
fn crlf_and_split_carriage_returns_preserve_live_text() {
    let mut dashboard = Dashboard::default();
    dashboard.push("tool", "hello\r\n");
    assert_eq!(dashboard.records()[0].text, "hello");
    dashboard.push("tool", "progress");
    dashboard.push("tool", "\r");
    dashboard.push("tool", "\n");
    assert_eq!(dashboard.records()[1].text, "progress");
    dashboard.push("tool", "old\rnew\n");
    assert_eq!(dashboard.records()[2].text, "new");
}

#[test]
fn hyperlink_and_color_escapes_can_cross_output_chunks() {
    let mut dashboard = Dashboard::default();
    dashboard.push("tool", "\x1b]8;;https://example.test\x1b");
    dashboard.push("tool", "\\link\x1b]8;;\x1b\\\nnext\n");
    assert_eq!(dashboard.records().len(), 2);
    assert_eq!(dashboard.records()[0].text, "link");
    assert_eq!(dashboard.records()[1].text, "next");
    dashboard.push("tool", "hello \x1b");
    dashboard.push("tool", "[31mred\x1b[0m\n");
    assert_eq!(dashboard.records()[2].text, "hello red");
}
