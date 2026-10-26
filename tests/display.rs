#![cfg(test)]

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Color, Modifier};
use std::path::PathBuf;
use std::time::{Duration, Instant};
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

fn rendered(dashboard: &mut Dashboard, jobs: &[Job]) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| sup::display::render(frame, dashboard, jobs))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn text_cell<'a>(buffer: &'a Buffer, text: &str) -> &'a Cell {
    for row in 0..buffer.area.height {
        let line: String = (0..buffer.area.width)
            .map(|column| buffer[(column, row)].symbol())
            .collect();
        if let Some(offset) = line.find(text) {
            return &buffer[(line[..offset].chars().count() as u16, row)];
        }
    }

    panic!("missing text: {text}");
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
    assert!(dashboard.pane("alpha").is_some());
    dashboard.expire_completed(Instant::now() + Duration::from_millis(500));
    assert!(dashboard.pane("alpha").is_none());
    assert_eq!(dashboard.focused_job(), Some("beta"));
    output(&mut dashboard, "beta", &format!("{}\n", "expanded".repeat(8)));
    let rows = screen(&mut dashboard, &jobs, 80, 24);
    assert!(!rows.iter().any(|row| row.contains("ALPHA-ONLY")));
    assert!(rows.iter().any(|row| row.contains(&"expanded".repeat(8))));
    assert!(rows.iter().any(|row| row.contains("failed")));
}

#[test]
fn completion_overlay_covers_finished_output_with_status_color() {
    for (status, label, color) in [
        (JobStatus::Succeeded, "✓ succeeded", Color::Rgb(158, 206, 106)),
        (JobStatus::Failed, "✗ failed", Color::Rgb(247, 118, 142)),
    ] {
        let jobs = [job("done"), job("running")];
        let mut dashboard = started(&jobs);
        output(&mut dashboard, "done", "FINISHED-OUTPUT\n");
        output(&mut dashboard, "running", "STILL-RUNNING\n");
        finish(&mut dashboard, &jobs[0], status);
        assert!(dashboard.pane("done").is_some());
        let buffer = rendered(&mut dashboard, &jobs);
        let overlay = text_cell(&buffer, label);
        assert_eq!(overlay.fg, color);
        assert_eq!(overlay.bg, Color::Rgb(36, 40, 59));
        assert!(overlay.modifier.contains(Modifier::BOLD));
        text_cell(&buffer, "STILL-RUNNING");
        let frame: String = buffer.content.iter().map(Cell::symbol).collect();
        assert!(!frame.contains("FINISHED-OUTPUT"));
    }
}

#[test]
fn completion_overlay_handles_tiny_and_narrow_terminals() {
    let jobs = [job("done")];
    let mut dashboard = started(&jobs);
    finish(&mut dashboard, &jobs[0], JobStatus::Succeeded);
    for (width, height) in [(1, 1), (4, 3), (12, 8), (20, 60), (160, 6)] {
        assert_eq!(screen(&mut dashboard, &jobs, width, height).len(), height as usize);
    }
}

#[test]
fn regression_inactive_pane_labels_keep_readable_contrast() {
    let jobs = [job("alpha"), job("beta")];
    let mut dashboard = started(&jobs);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| sup::display::render(frame, &mut dashboard, &jobs))
        .unwrap();
    let buffer = terminal.backend().buffer();
    for label in ["Label beta (beta)", "Label alpha (alpha)"] {
        let (column, row) = (0..24)
            .find_map(|row| {
                let line: String = (0..80).map(|column| buffer[(column, row)].symbol()).collect();
                line.find(label)
                    .map(|offset| (line[..offset].chars().count() as u16, row))
            })
            .unwrap();
        assert_eq!(buffer[(column, row)].fg, Color::Rgb(192, 202, 245), "{label}");
    }
}

#[test]
fn regression_pane_output_keeps_command_colors() {
    let jobs = [job("build")];
    let mut dashboard = started(&jobs);
    output(&mut dashboard, "build", "\x1b[31mred\x1b[0m default\n");
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| sup::display::render(frame, &mut dashboard, &jobs))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let (column, row) = (0..24)
        .find_map(|row| {
            let line: String = (0..80).map(|column| buffer[(column, row)].symbol()).collect();
            line.find("red default")
                .map(|offset| (line[..offset].chars().count() as u16, row))
        })
        .unwrap();
    assert_eq!(buffer[(column, row)].fg, Color::Red);
    assert_eq!(buffer[(column + 4, row)].fg, Color::Rgb(192, 202, 245));
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
    dashboard.expire_completed(Instant::now() + Duration::from_millis(500));
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
    dashboard.expire_completed(Instant::now() + Duration::from_millis(500));
    let rows = screen(&mut dashboard, &jobs, 80, 24);
    assert_eq!(dashboard.focused_job(), Some("two"));
    assert!(rows[23].contains("1/1"));
    assert!(dashboard.pane("preflight").is_none());
    finish(&mut dashboard, &jobs[0], JobStatus::Skipped);
    finish(&mut dashboard, &jobs[1], JobStatus::Succeeded);
    dashboard.expire_completed(Instant::now() + Duration::from_millis(500));
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
        .map(sup::display::OutputRecord::storage_bytes)
        .sum();
    assert!(bytes <= 8 * 1024 * 1024);
    assert!(bytes > 8 * 1024 * 1024 - 32_000);
    finish(&mut dashboard, &jobs[0], JobStatus::Failed);
    dashboard.expire_completed(Instant::now() + Duration::from_millis(500));
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

#[test]
fn streaming_styles_persist_across_chunks_lines_and_stay_per_job() {
    let jobs = [job("alpha"), job("beta")];
    let mut dashboard = started(&jobs);
    output(&mut dashboard, "alpha", "\x1b[3");
    output(&mut dashboard, "beta", "PLAIN\n");
    output(&mut dashboard, "alpha", "1;1;4mRED\nCAR");
    output(&mut dashboard, "alpha", "RY\x1b[22;24;39mDEFAULT\n");
    let buffer = rendered(&mut dashboard, &jobs);
    for text in ["RED", "CARRY"] {
        let cell = text_cell(&buffer, text);
        assert_eq!(cell.fg, Color::Red);
        assert!(cell.modifier.contains(Modifier::BOLD | Modifier::UNDERLINED));
    }

    for text in ["PLAIN", "DEFAULT"] {
        let cell = text_cell(&buffer, text);
        assert_eq!(cell.fg, Color::Rgb(192, 202, 245));
        assert!(!cell.modifier.intersects(Modifier::BOLD | Modifier::UNDERLINED));
    }
}

struct ColorSample {
    sequence: &'static str,
    label: &'static str,
    foreground: Color,
    background: Color,
    modifiers: Modifier,
}

#[test]
fn rendered_palette_backgrounds_modifiers_and_resets_match_sgr() {
    let jobs = [job("tool")];
    let mut dashboard = started(&jobs);
    let samples = [
        ColorSample {
            sequence: "\x1b[31;44m",
            label: "BASIC",
            foreground: Color::Red,
            background: Color::Blue,
            modifiers: Modifier::empty(),
        },
        ColorSample {
            sequence: "\x1b[96;101m",
            label: "BRIGHT",
            foreground: Color::LightCyan,
            background: Color::LightRed,
            modifiers: Modifier::empty(),
        },
        ColorSample {
            sequence: "\x1b[38;5;202;48;5;17m",
            label: "INDEXED",
            foreground: Color::Indexed(202),
            background: Color::Indexed(17),
            modifiers: Modifier::empty(),
        },
        ColorSample {
            sequence: "\x1b[38;2;10;20;30;48;2;40;50;60m",
            label: "TRUECOLOR",
            foreground: Color::Rgb(10, 20, 30),
            background: Color::Rgb(40, 50, 60),
            modifiers: Modifier::empty(),
        },
        ColorSample {
            sequence: "\x1b[38:2::70:80:90;48:5:19m",
            label: "COLON",
            foreground: Color::Rgb(70, 80, 90),
            background: Color::Indexed(19),
            modifiers: Modifier::empty(),
        },
        ColorSample {
            sequence: "\x1b[1;2;3;4;5;6;7;9m",
            label: "MODIFIERS",
            foreground: Color::Rgb(192, 202, 245),
            background: Color::Rgb(26, 27, 38),
            modifiers: Modifier::BOLD
                .union(Modifier::DIM)
                .union(Modifier::ITALIC)
                .union(Modifier::UNDERLINED)
                .union(Modifier::SLOW_BLINK)
                .union(Modifier::RAPID_BLINK)
                .union(Modifier::REVERSED)
                .union(Modifier::CROSSED_OUT),
        },
    ];
    for sample in &samples {
        output(
            &mut dashboard,
            "tool",
            &format!("{}{}\x1b[0m\n", sample.sequence, sample.label),
        );
    }

    output(
        &mut dashboard,
        "tool",
        "\x1b[31;44;1;2;3;4;5;6;7;8;9m\x1b[39;49;22;23;24;25;27;28;29mOFF\n",
    );
    let buffer = rendered(&mut dashboard, &jobs);
    for sample in &samples {
        let cell = text_cell(&buffer, sample.label);
        assert_eq!(cell.fg, sample.foreground, "{}", sample.label);
        assert_eq!(cell.bg, sample.background, "{}", sample.label);
        assert_eq!(cell.modifier, sample.modifiers, "{}", sample.label);
    }

    let reset = text_cell(&buffer, "OFF");
    assert_eq!(reset.fg, Color::Rgb(192, 202, 245));
    assert_eq!(reset.bg, Color::Rgb(26, 27, 38));
    assert_eq!(reset.modifier, Modifier::empty());
}

#[test]
fn progress_replaces_text_and_styles_and_scroll_keeps_original_colors() {
    let jobs: Vec<Job> = (0..5).map(|index| job(&format!("job{index}"))).collect();
    let mut dashboard = started(&jobs);
    output(&mut dashboard, "job4", "\x1b[31mOLD\r");
    output(&mut dashboard, "job4", "\x1b[32mNEW\r");
    output(&mut dashboard, "job4", "\nPART");
    rendered(&mut dashboard, &jobs);
    assert!(dashboard.pane("job4").unwrap().records()[0].text == "NEW");
    dashboard.change_page(99);
    let buffer = rendered(&mut dashboard, &jobs);
    assert_eq!(text_cell(&buffer, "NEW").fg, Color::Green);
    assert_eq!(text_cell(&buffer, "PART").fg, Color::Green);
    dashboard.scroll(-1);
    for index in 0..80 {
        output(&mut dashboard, "job4", &format!("\n\x1b[34mrow-{index}"));
    }

    let buffer = rendered(&mut dashboard, &jobs);
    assert_eq!(text_cell(&buffer, "NEW").fg, Color::Green);
    dashboard.resume();
    let buffer = rendered(&mut dashboard, &jobs);
    assert_eq!(text_cell(&buffer, "row-79").fg, Color::Blue);
}

#[test]
fn control_strings_malformed_and_long_sequences_do_not_reach_the_screen() {
    let jobs = [job("tool")];
    let mut dashboard = started(&jobs);
    output(&mut dashboard, "tool", "\x1b]52;c;SECRET");
    output(&mut dashboard, "tool", &"ignored".repeat(100_000));
    output(
        &mut dashboard,
        "tool",
        "\x1b\\\x1bPignored\x1b\\\x1b^ignored\x1b\\\x1b_ignored\x1b\\\x1b[2J\x1b[H",
    );
    output(&mut dashboard, "tool", &format!("\x1b[{}mSAFE", "31;".repeat(100_000)));
    output(&mut dashboard, "tool", "\x1b[38;2;999;0;0mRANGE\x1b[?31mPRIVATE\n");
    let records = dashboard.pane("tool").unwrap().records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].text, "SAFERANGEPRIVATE");
    assert!(records[0].storage_bytes() < 1024);
    let buffer = rendered(&mut dashboard, &jobs);
    assert_eq!(text_cell(&buffer, "SAFE").fg, Color::Rgb(192, 202, 245));
    assert_eq!(text_cell(&buffer, "RANGE").fg, Color::Rgb(192, 202, 245));
    assert_eq!(text_cell(&buffer, "PRIVATE").fg, Color::Rgb(192, 202, 245));
}

#[test]
fn style_metadata_and_repeated_partial_events_share_output_budgets() {
    let jobs = [job("tool")];
    let mut dashboard = started(&jobs);
    let many_styles: String = (0..2_000).map(|index| format!("\x1b[38;5;{}mx", index % 256)).collect();
    for _ in 0..20 {
        output(&mut dashboard, "tool", &many_styles);
    }

    let record = &dashboard.pane("tool").unwrap().records()[0];
    assert!(record.text.contains("event truncated"));
    assert!(record.text.len() <= 16 * 1024);
    assert!(record.storage_bytes() > record.text.len());
    output(&mut dashboard, "tool", "\r\x1b[32mRECOVERED\n");
    assert_eq!(dashboard.pane("tool").unwrap().records()[0].text, "RECOVERED");
    for _ in 0..800 {
        output(&mut dashboard, "tool", &format!("{many_styles}\n"));
    }

    let bytes: usize = dashboard
        .pane("tool")
        .unwrap()
        .records()
        .iter()
        .map(sup::display::OutputRecord::storage_bytes)
        .sum();
    assert!(bytes <= 8 * 1024 * 1024);
    assert!(bytes > 8 * 1024 * 1024 - 200_000);
    let text_bytes: usize = dashboard
        .pane("tool")
        .unwrap()
        .records()
        .iter()
        .map(|record| record.text.len())
        .sum();
    assert!(text_bytes < bytes / 2);
}

#[test]
fn truncated_partial_line_recovers_after_newline_in_next_event() {
    let jobs = [job("tool")];
    let mut dashboard = started(&jobs);
    output(&mut dashboard, "tool", &"x".repeat(16 * 1024));
    output(&mut dashboard, "tool", "overflow");
    assert!(
        dashboard.pane("tool").unwrap().records()[0]
            .text
            .contains("event truncated")
    );
    output(&mut dashboard, "tool", "\nNEXT\n");
    let records = dashboard.pane("tool").unwrap().records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].text, "NEXT");
    assert!(records[0].text.len() <= 16 * 1024);
}

#[test]
fn repeated_truncated_lines_recover_without_growing_old_records_or_losing_style() {
    let jobs = [job("tool")];
    let mut dashboard = started(&jobs);
    let large = "x".repeat(20 * 1024);
    output(&mut dashboard, "tool", "\x1b[31m");
    output(&mut dashboard, "tool", &large);
    let original = dashboard.pane("tool").unwrap().records()[0].clone();
    for index in 0..32 {
        output(&mut dashboard, "tool", &format!("\nNEXT-{index}\n"));
        assert_eq!(
            dashboard.pane("tool").unwrap().records().back().unwrap().text,
            format!("NEXT-{index}")
        );
        output(&mut dashboard, "tool", &large);
    }

    output(&mut dashboard, "tool", "\nTAIL\n");
    let records = dashboard.pane("tool").unwrap().records();
    assert_eq!(records.len(), 66);
    assert_eq!(records[0], original);
    assert!(records.iter().all(|record| record.text.len() <= 16 * 1024));
    assert_eq!(text_cell(&rendered(&mut dashboard, &jobs), "TAIL").fg, Color::Red);
}

#[test]
fn recovery_keeps_new_text_budget_across_line_boundaries() {
    let jobs = [job("tool")];
    let mut dashboard = started(&jobs);
    output(&mut dashboard, "tool", &"x".repeat(16 * 1024));
    let original = dashboard.pane("tool").unwrap().records()[0].clone();
    output(&mut dashboard, "tool", &format!("\n{}", "ROW\n".repeat(10_000)));
    let records = dashboard.pane("tool").unwrap().records();
    assert_eq!(records[0], original);
    assert_eq!(records[1].text, "ROW");
    let new_bytes: usize = records.iter().skip(1).map(|record| record.text.len()).sum();
    assert!(new_bytes <= 16 * 1024);
    assert!(records.back().unwrap().text.contains("event truncated"));
}

#[test]
fn pipe_sized_plain_multiline_chunk_keeps_every_line() {
    let jobs = [job("tool")];
    let mut dashboard = started(&jobs);
    let payload: String = (0..200)
        .map(|index| format!("line-{index:03} {}\n", "x".repeat(70)))
        .collect();
    assert!(payload.len() <= 16 * 1024);
    output(&mut dashboard, "tool", &payload);
    let records = dashboard.pane("tool").unwrap().records();
    assert_eq!(records.len(), 200);
    assert_eq!(records.back().unwrap().text, format!("line-199 {}", "x".repeat(70)));
    assert!(records.iter().all(|record| !record.text.contains("event truncated")));
}

#[test]
fn pipe_sized_colored_multiline_chunk_keeps_every_line() {
    let jobs = [job("tool")];
    let mut dashboard = started(&jobs);
    let payload: String = (0..200)
        .map(|index| format!("\x1b[32mrow-{index:03} {}\x1b[0m\n", "x".repeat(60)))
        .collect();
    assert!(payload.len() <= 16 * 1024);
    output(&mut dashboard, "tool", &payload);
    let records = dashboard.pane("tool").unwrap().records();
    assert_eq!(records.len(), 200);
    assert!(records.iter().all(|record| !record.text.contains("event truncated")));
    assert_eq!(text_cell(&rendered(&mut dashboard, &jobs), "row-199").fg, Color::Green);
}

#[test]
fn pipe_sized_empty_line_burst_uses_record_retention_without_false_truncation() {
    let jobs = [job("tool")];
    let mut dashboard = started(&jobs);
    output(&mut dashboard, "tool", &"\n".repeat(16 * 1024));
    let records = dashboard.pane("tool").unwrap().records();
    assert_eq!(records.len(), 10_000);
    assert!(records.iter().all(|record| record.text.is_empty()));
    output(&mut dashboard, "tool", "AFTER\n");
    assert_eq!(dashboard.pane("tool").unwrap().records().back().unwrap().text, "AFTER");
}

#[test]
fn oversized_empty_line_event_counts_newlines_toward_text_budget() {
    let jobs = [job("tool")];
    let mut dashboard = started(&jobs);
    output(&mut dashboard, "tool", &format!("{}\nLAST\n", "\n".repeat(100_000)));
    let records = dashboard.pane("tool").unwrap().records();
    assert_eq!(records.len(), 10_000);
    assert!(records.back().unwrap().text.contains("event truncated"));
    assert!(records.iter().all(|record| !record.text.contains("LAST")));
}

#[test]
fn partial_line_cap_does_not_drop_following_lines_in_same_pipe_chunk() {
    let jobs = [job("tool")];
    let mut dashboard = started(&jobs);
    output(&mut dashboard, "tool", &"x".repeat(16_000));
    output(
        &mut dashboard,
        "tool",
        &format!("{}\n\x1b[32mNEXT\n", "é".repeat(1_000)),
    );
    let records = dashboard.pane("tool").unwrap().records();
    assert_eq!(records.len(), 2);
    assert!(records[0].text.contains("event truncated"));
    assert!(records[0].text.len() <= 16 * 1024);
    assert_eq!(records[1].text, "NEXT");
    assert_eq!(text_cell(&rendered(&mut dashboard, &jobs), "NEXT").fg, Color::Green);
}

#[test]
fn full_pipe_read_with_utf8_carry_keeps_every_complete_line() {
    let jobs = [job("tool")];
    for carry in 0..=3 {
        let mut dashboard = started(&jobs);
        output(&mut dashboard, "tool", "OLD\r");
        let mut payload = "🚀\n".to_owned();
        let remaining = 16 * 1024 + carry - payload.len();
        payload.push_str(&"x\n".repeat(remaining / 2));
        if !remaining.is_multiple_of(2) {
            payload.push('z');
        }

        output(&mut dashboard, "tool", &payload);
        let actual: Vec<&str> = dashboard
            .pane("tool")
            .unwrap()
            .records()
            .iter()
            .map(|record| record.text.as_str())
            .collect();
        let expected: Vec<&str> = payload.split_terminator('\n').collect();
        assert_eq!(actual.len(), expected.len(), "UTF-8 carry: {carry}");
        assert_eq!(actual, expected, "UTF-8 carry: {carry}");
    }
}
