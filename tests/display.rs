#![cfg(test)]

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::path::PathBuf;
use std::time::Duration;
use sup::config::{AuthMode, Job};
use sup::display::Dashboard;
use sup::runner::{JobResult, JobStatus, RunEvent};

fn job(name: &str) -> Job {
    Job {
        name: name.to_owned(),
        label: format!("Label {name}"),
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

fn started(jobs: &[Job]) -> Dashboard {
    let mut dashboard = Dashboard::new(jobs);
    for job in jobs {
        dashboard.apply_event(RunEvent::Started(job.name.clone()));
    }

    dashboard
}

fn output(dashboard: &mut Dashboard, job: &str, text: &str) {
    dashboard.apply_event(RunEvent::Output {
        job: job.to_owned(),
        text: text.to_owned(),
    });
}

fn finish(dashboard: &mut Dashboard, job: &Job, status: JobStatus) {
    dashboard.apply_event(RunEvent::Finished(Box::new(JobResult {
        job: job.clone(),
        status,
        exit_code: None,
        elapsed: Duration::ZERO,
        log_path: None,
        reason: String::new(),
    })));
}

fn screen(dashboard: &mut Dashboard, jobs: &[Job], width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| sup::display::render(frame, dashboard, jobs))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|row| (0..width).map(|column| buffer[(column, row)].symbol()).collect())
        .collect()
}

#[test]
fn named_streams_stay_in_separate_panes_and_finished_panes_release_space() {
    let jobs = [job("alpha"), job("beta")];
    let mut dashboard = started(&jobs);
    output(&mut dashboard, "alpha", "ALPHA-ONLY\n");
    output(&mut dashboard, "beta", "BETA-ONLY\n");
    let rows = screen(&mut dashboard, &jobs, 80, 24);
    let alpha = rows.iter().find(|row| row.contains("ALPHA-ONLY")).unwrap();
    assert!(alpha.find("ALPHA-ONLY").unwrap() < 40);
    assert!(alpha.find("BETA-ONLY").unwrap() > 40);
    assert!(rows.iter().any(|row| row.contains("Label alpha (alpha)")));
    assert!(rows.iter().any(|row| row.contains("Label beta (beta)")));
    finish(&mut dashboard, &jobs[0], JobStatus::Failed);
    assert!(dashboard.pane("alpha").is_none());
    assert_eq!(dashboard.focused_job(), Some("beta"));
    output(&mut dashboard, "beta", &format!("{}\n", "expanded".repeat(8)));
    let rows = screen(&mut dashboard, &jobs, 80, 24);
    assert!(!rows.iter().any(|row| row.contains("ALPHA-ONLY")));
    assert!(rows.iter().any(|row| row.contains(&"expanded".repeat(8))));
    assert!(rows.iter().any(|row| row.contains("failed")));
}

#[test]
fn silent_and_out_of_order_jobs_use_configured_order() {
    let jobs = [job("first"), job("second")];
    let mut dashboard = Dashboard::new(&jobs);
    dashboard.apply_event(RunEvent::Started("second".to_owned()));
    dashboard.apply_event(RunEvent::Started("first".to_owned()));
    let rows = screen(&mut dashboard, &jobs, 80, 24);
    let headings = rows.iter().find(|row| row.contains("Label first (first)")).unwrap();
    assert!(headings.find("Label first").unwrap() < headings.find("Label second").unwrap());
    assert_eq!(dashboard.focused_job(), Some("second"));
}

#[test]
fn tall_dashboard_shows_more_than_ten_records_and_follow_footer() {
    let jobs = [job("build")];
    let mut dashboard = started(&jobs);
    for index in 0..30 {
        output(&mut dashboard, "build", &format!("record-{index}\n"));
    }

    let rows = screen(&mut dashboard, &jobs, 120, 60);
    assert!(rows.iter().filter(|row| row.contains("record-")).count() > 10);
    assert!(rows.iter().rev().take(3).any(|row| row.contains("record-29")));
    assert!(rows[59].contains("End follow"));
}

#[test]
fn partial_lines_progress_and_split_escapes_remain_per_job() {
    let jobs = [job("tool"), job("other")];
    let mut dashboard = started(&jobs);
    output(&mut dashboard, "tool", "first\npart");
    output(&mut dashboard, "other", "independent\n");
    output(&mut dashboard, "tool", "ial\r\n20% complete");
    output(&mut dashboard, "tool", "\r\x1b[31m40% complete\x1b[0m");
    let records = dashboard.pane("tool").unwrap().records();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].text, "first");
    assert_eq!(records[1].text, "partial");
    assert_eq!(records[2].text, "40% complete");
    output(&mut dashboard, "tool", "\r");
    output(&mut dashboard, "tool", "\n\x1b]8;;https://example.test\x1b");
    output(&mut dashboard, "tool", "\\link\x1b]8;;\x1b\\\nhello \x1b");
    output(&mut dashboard, "tool", "[31mred\x1b[0m\n");
    assert_eq!(dashboard.pane("tool").unwrap().records()[3].text, "link");
    assert_eq!(dashboard.pane("tool").unwrap().records()[4].text, "hello red");
    assert_eq!(dashboard.pane("other").unwrap().records()[0].text, "independent");
    output(&mut dashboard, "other", "progress 20%");
    assert!(screen(&mut dashboard, &jobs, 80, 18).concat().contains("progress 20%"));
}

#[test]
fn paging_captures_hidden_output_and_clamps_at_ends() {
    let jobs: Vec<Job> = (0..5).map(|index| job(&format!("job{index}"))).collect();
    let mut dashboard = started(&jobs);
    let rows = screen(&mut dashboard, &jobs, 80, 24);
    assert!(rows[23].contains("1/3 · 3 hidden"));
    output(&mut dashboard, "job4", "hidden output\n");
    assert_eq!(dashboard.focused_job(), Some("job0"));
    dashboard.change_page(1);
    assert_eq!(dashboard.focused_job(), Some("job2"));
    dashboard.cycle_focus(true);
    assert_eq!(dashboard.focused_job(), Some("job3"));
    dashboard.change_page(1);
    let rows = screen(&mut dashboard, &jobs, 80, 24);
    assert!(rows.concat().contains("hidden output"));
    dashboard.change_page(1);
    assert_eq!(dashboard.focused_job(), Some("job4"));
    dashboard.change_page(-99);
    assert_eq!(dashboard.focused_job(), Some("job0"));
    dashboard.cycle_focus(false);
    dashboard.change_page(-1);
    assert_eq!(dashboard.focused_job(), Some("job1"));
}

#[test]
fn independent_scroll_anchors_survive_output_reflow_and_resize() {
    let jobs = [job("alpha"), job("beta"), job("gamma")];
    let mut dashboard = started(&jobs);
    for index in 0..50 {
        for job in &jobs {
            output(&mut dashboard, &job.name, &format!("{}-{index}\n", job.name));
        }
    }

    screen(&mut dashboard, &jobs, 80, 24);
    dashboard.scroll(-20);
    dashboard.cycle_focus(false);
    assert_eq!(dashboard.focused_job(), Some("beta"));
    dashboard.scroll(-10);
    let rows = screen(&mut dashboard, &jobs, 80, 24);
    assert!(rows.concat().contains("alpha-29"));
    assert!(rows.concat().contains("beta-39"));
    dashboard.resume();
    assert!(!dashboard.pane("alpha").unwrap().is_following());
    assert!(dashboard.pane("beta").unwrap().is_following());
    finish(&mut dashboard, &jobs[0], JobStatus::Succeeded);
    assert_eq!(dashboard.focused_job(), Some("beta"));
    screen(&mut dashboard, &jobs, 40, 18);
    assert_eq!(dashboard.focused_job(), Some("beta"));
    output(&mut dashboard, "gamma", "latest\n");
    assert_eq!(dashboard.focused_job(), Some("beta"));
}

#[test]
fn closing_last_pane_on_page_selects_previous_and_prestart_failures_create_no_pane() {
    let jobs = [job("one"), job("two"), job("three"), job("preflight")];
    let mut dashboard = Dashboard::new(&jobs);
    for job in &jobs[..3] {
        dashboard.apply_event(RunEvent::Started(job.name.clone()));
    }

    screen(&mut dashboard, &jobs, 80, 24);
    dashboard.change_page(1);
    assert_eq!(dashboard.focused_job(), Some("three"));
    finish(&mut dashboard, &jobs[2], JobStatus::Cancelled);
    finish(&mut dashboard, &jobs[3], JobStatus::Failed);
    let rows = screen(&mut dashboard, &jobs, 80, 24);
    assert_eq!(dashboard.focused_job(), Some("two"));
    assert!(rows[23].contains("1/1"));
    assert!(dashboard.pane("preflight").is_none());
    finish(&mut dashboard, &jobs[0], JobStatus::Skipped);
    finish(&mut dashboard, &jobs[1], JobStatus::Succeeded);
    assert!(
        screen(&mut dashboard, &jobs, 80, 24)
            .concat()
            .contains("finished panes close")
    );
}

#[test]
fn history_caps_aggregate_records_bytes_and_each_event_then_releases_on_finish() {
    let jobs = [job("alpha"), job("beta")];
    let mut dashboard = started(&jobs);
    output(&mut dashboard, "alpha", &format!("{}\n", "x".repeat(20 * 1024)));
    assert!(
        dashboard.pane("alpha").unwrap().records()[0]
            .text
            .contains("event truncated")
    );
    for index in 0..11_000 {
        output(&mut dashboard, if index % 2 == 0 { "alpha" } else { "beta" }, "row\n");
    }

    let total: usize = jobs
        .iter()
        .map(|job| dashboard.pane(&job.name).unwrap().records().len())
        .sum();
    assert_eq!(total, 10_000);
    for index in 0..1_000 {
        output(
            &mut dashboard,
            if index % 2 == 0 { "alpha" } else { "beta" },
            &format!("{}\n", "z".repeat(16_000)),
        );
    }

    let bytes: usize = jobs
        .iter()
        .flat_map(|job| dashboard.pane(&job.name).unwrap().records())
        .map(|record| record.text.len())
        .sum();
    assert!(bytes <= 8 * 1024 * 1024);
    assert!(bytes > 8 * 1024 * 1024 - 32_000);
    finish(&mut dashboard, &jobs[0], JobStatus::Failed);
    assert!(dashboard.pane("alpha").is_none());
    output(&mut dashboard, "beta", "still works\n");
    assert_eq!(
        dashboard.pane("beta").unwrap().records().back().unwrap().text,
        "still works"
    );
}

#[test]
fn eviction_of_entire_pane_allows_fresh_partial_output() {
    let jobs = [job("old"), job("live")];
    let mut dashboard = started(&jobs);
    output(&mut dashboard, "old", "oldest\n");
    output(&mut dashboard, "live", "partial");
    for _ in 0..10_000 {
        output(&mut dashboard, "old", "row\n");
    }

    output(&mut dashboard, "live", "new partial");
    assert_eq!(
        dashboard.pane("live").unwrap().records().back().unwrap().text,
        "new partial"
    );
    output(&mut dashboard, "live", " complete\n");
    assert_eq!(
        dashboard.pane("live").unwrap().records().back().unwrap().text,
        "new partial complete"
    );
}

#[test]
fn eviction_of_old_records_preserves_another_panes_partial_line_and_scroll_anchor() {
    let jobs = [job("old"), job("live")];
    let mut dashboard = started(&jobs);
    for index in 0..10_000 {
        output(&mut dashboard, "old", &format!("row-{index}\n"));
    }

    screen(&mut dashboard, &jobs, 80, 24);
    dashboard.scroll(-10);
    output(&mut dashboard, "live", "partial");
    for _ in 0..10 {
        output(&mut dashboard, "old", "newer\n");
    }

    output(&mut dashboard, "live", " complete\n");
    assert_eq!(dashboard.pane("live").unwrap().records()[0].text, "partial complete");
    assert!(!dashboard.pane("old").unwrap().is_following());
    assert!(screen(&mut dashboard, &jobs, 80, 24).concat().contains("row-9989"));
}

#[test]
fn tiny_dimensions_and_representative_frames() {
    let jobs: Vec<Job> = (0..7).map(|index| job(&format!("job{index}"))).collect();
    let mut dashboard = started(&jobs);
    for job in &jobs {
        output(&mut dashboard, &job.name, &format!("{} independent output\n", job.name));
    }

    for (width, height) in [
        (1, 1),
        (4, 3),
        (12, 8),
        (20, 60),
        (160, 6),
        (80, 18),
        (80, 24),
        (160, 50),
    ] {
        let rows = screen(&mut dashboard, &jobs, width, height);
        assert_eq!(rows.len(), height as usize);
        if [(80, 24), (160, 50)].contains(&(width, height)) {
            // Optional evidence capture keeps ordinary tests free of workspace writes.
            if let Ok(directory) = std::env::var("SUP_SCREEN_EVIDENCE") {
                std::fs::write(
                    PathBuf::from(directory).join(format!("{width}x{height}.txt")),
                    format!("{}\n", rows.join("\n")),
                )
                .unwrap();
            }
        }
    }
}

#[test]
fn narrow_tall_and_wide_short_terminals_show_one_clipped_pane() {
    let jobs = [job("alpha"), job("beta"), job("gamma")];
    let mut dashboard = started(&jobs);
    for (width, height) in [(20, 60), (160, 6)] {
        screen(&mut dashboard, &jobs, width, height);
        dashboard.cycle_focus(false);
        assert_eq!(dashboard.focused_job(), Some("alpha"));
        dashboard.change_page(1);
        assert_eq!(dashboard.focused_job(), Some("beta"));
        dashboard.change_page(-1);
    }
}

#[test]
fn focused_title_marker_follows_tab_selection() {
    let jobs = [job("alpha"), job("beta")];
    let mut dashboard = started(&jobs);
    let initial = screen(&mut dashboard, &jobs, 80, 24).concat();
    assert!(initial.contains("* Label alpha (alpha)"));
    assert!(!initial.contains("* Label beta (beta)"));
    assert_eq!(initial.matches("* Label").count(), 1);
    dashboard.cycle_focus(false);
    let selected = screen(&mut dashboard, &jobs, 80, 24).concat();
    assert!(selected.contains("* Label beta (beta)"));
    assert!(!selected.contains("* Label alpha (alpha)"));
    assert_eq!(selected.matches("* Label").count(), 1);
}

#[test]
fn grid_minimum_boundaries_change_rendered_pane_count() {
    let jobs = [job("alpha"), job("beta"), job("gamma")];
    let mut dashboard = started(&jobs);
    for (width, height, expected) in [(35, 50, 1), (36, 50, 3), (80, 9, 1), (80, 10, 2)] {
        let frame = screen(&mut dashboard, &jobs, width, height).concat();
        let pane_headings = jobs
            .iter()
            .filter(|job| frame.contains(&format!("({})", job.name)))
            .count();
        assert_eq!(pane_headings, expected, "{width}x{height}: {frame}");
    }
}
