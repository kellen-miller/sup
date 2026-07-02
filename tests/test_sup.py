import io
import tempfile
import unittest
from contextlib import redirect_stdout
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest.mock import patch

from rich.console import Console

from sup.cli import main
from sup.display import LiveDashboard, progress_bar, render_dry_run, render_summary
from sup.jobs import config_path, load_jobs_config, resolve_job_selection
from sup.logs import cleanup_old_runs, create_run_dir, tail_lines
from sup.runner import CommandResult, JobResult, Runner


class SelectionTest(unittest.TestCase):
    def test_default_jobs_preserve_core_then_parallel_order(self):
        jobs_config = load_jobs_config(config_path())
        jobs = jobs_config.jobs

        self.assertEqual(
            [job.name for job in jobs if job.phase == "core"],
            ["brew-upgrade", "brew-cleanup", "zimfw-upgrade"],
        )
        self.assertEqual(
            [job.name for job in jobs if job.phase == "parallel"],
            [
                "zimfw-update",
                "gup",
                "gcloud",
                "mas",
                "npm",
                "pnpm",
                "rustup",
                "cargo-install-update",
                "skills",
            ],
        )

    def test_default_config_path_uses_config_yaml(self):
        self.assertEqual(config_path().name, "config.yaml")

    def test_selection_accepts_exact_names_and_aliases(self):
        jobs_config = load_jobs_config(config_path())
        jobs = jobs_config.jobs

        selected = resolve_job_selection(
            jobs, aliases=jobs_config.aliases, only=["skills", "rustup"], skip=[]
        )
        self.assertEqual([job.name for job in selected], ["rustup", "skills"])

        selected = resolve_job_selection(
            jobs, aliases=jobs_config.aliases, only=[], skip=["brew", "mas"]
        )
        names = [job.name for job in selected]
        self.assertNotIn("brew-upgrade", names)
        self.assertNotIn("brew-cleanup", names)
        self.assertNotIn("mas", names)
        self.assertIn("zimfw-upgrade", names)

    def test_selection_rejects_unknown_selector(self):
        with self.assertRaisesRegex(ValueError, "unknown selector"):
            jobs_config = load_jobs_config(config_path())
            resolve_job_selection(
                jobs_config.jobs,
                aliases=jobs_config.aliases,
                only=["missing"],
                skip=[],
            )

    def test_loads_custom_jobs_from_yaml_with_home_expansion(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            job_file = root / "config.yaml"
            job_file.write_text(
                """
aliases:
  mine:
    - custom
jobs:
  - name: custom
    label: Custom Job
    phase: parallel
    command: ["$HOME/bin/custom", "upgrade"]
    required_paths: ["$HOME/bin/custom"]
    optional: true
    log_name: custom.log
""",
                encoding="utf-8",
            )

            jobs_config = load_jobs_config(job_file, home=root)

        self.assertEqual(jobs_config.aliases, {"mine": ("custom",)})
        self.assertEqual(
            jobs_config.jobs[0].command, (str(root / "bin" / "custom"), "upgrade")
        )
        self.assertEqual(jobs_config.jobs[0].required_paths, (root / "bin" / "custom",))

    def test_loads_custom_jobs_from_yaml_with_env_expansion(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            updater = root / "bin" / "update-skills"
            job_file = root / "config.yaml"
            job_file.write_text(
                """
jobs:
  - name: env-job
    phase: parallel
    command: ["$SUP_SKILLS_UPDATE", "--quiet"]
    required_env: ["SUP_SKILLS_UPDATE"]
    optional: true
""",
                encoding="utf-8",
            )

            jobs_config = load_jobs_config(
                job_file,
                home=root,
                env={"SUP_SKILLS_UPDATE": str(updater)},
            )

        self.assertEqual(jobs_config.jobs[0].command, (str(updater), "--quiet"))
        self.assertEqual(jobs_config.jobs[0].required_env, ("SUP_SKILLS_UPDATE",))

    def test_config_validation_rejects_duplicate_job_names(self):
        with tempfile.TemporaryDirectory() as tmp:
            job_file = Path(tmp) / "config.yaml"
            job_file.write_text(
                """
jobs:
  - name: duplicate
    phase: parallel
    command: ["one"]
  - name: duplicate
    phase: parallel
    command: ["two"]
""",
                encoding="utf-8",
            )

            with self.assertRaisesRegex(ValueError, "duplicate job"):
                load_jobs_config(job_file)


class LogTest(unittest.TestCase):
    def test_create_run_dir_uses_home_cache(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)

            run_dir = create_run_dir(home, timestamp="20260702T100000Z")

        self.assertEqual(
            run_dir,
            home / ".cache" / "sup" / "runs" / "20260702T100000Z",
        )

    def test_cleanup_old_runs_preserves_current_and_recent_runs(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            runs = root / ".cache" / "sup" / "runs"
            old = runs / "old"
            recent = runs / "recent"
            current = runs / "current"
            old.mkdir(parents=True)
            recent.mkdir()
            current.mkdir()
            now = datetime(2026, 7, 2, tzinfo=timezone.utc)
            old_time = (now - timedelta(days=31)).timestamp()
            recent_time = (now - timedelta(days=3)).timestamp()
            for path, mtime in (
                (old, old_time),
                (recent, recent_time),
                (current, old_time),
            ):
                path.touch()
                path.chmod(0o755)
                import os

                os.utime(path, (mtime, mtime))

            removed = cleanup_old_runs(root, current, retention_days=30, now=now)

            self.assertEqual(removed, [old])
            self.assertFalse(old.exists())
            self.assertTrue(recent.exists())
            self.assertTrue(current.exists())

    def test_tail_lines_returns_last_n_lines(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "job.log"
            log.write_text("one\ntwo\nthree\nfour\n", encoding="utf-8")

            self.assertEqual(tail_lines(log, 2), ["three", "four"])


class RunnerTest(unittest.TestCase):
    def test_dry_run_succeeds_without_running_commands(self):
        runner = Runner(home=Path("/tmp/example-home"), command_runner=lambda job: None)

        jobs = load_jobs_config(config_path()).jobs
        results = runner.run(
            [job for job in jobs if job.name == "skills"], dry_run=True
        )

        self.assertEqual(results[0].status, "succeeded")
        self.assertEqual(results[0].reason, "dry run")

    def test_missing_optional_command_is_skipped(self):
        jobs = [
            job for job in load_jobs_config(config_path()).jobs if job.name == "gup"
        ]
        runner = Runner(
            home=Path("/tmp/example-home"),
            command_exists=lambda name: False,
            path_exists=lambda path: False,
        )

        results = runner.run(jobs, dry_run=False)

        self.assertEqual(results[0].status, "skipped")
        self.assertIn("missing optional", results[0].reason)

    def test_missing_optional_env_is_skipped(self):
        jobs = [
            job
            for job in load_jobs_config(config_path(), env={}).jobs
            if job.name == "skills"
        ]
        runner = Runner(
            home=Path("/tmp/example-home"),
            command_exists=lambda name: True,
            path_exists=lambda path: True,
            env={},
            command_runner=lambda job: CommandResult(exit_code=0),
        )

        results = runner.run(jobs, dry_run=False)

        self.assertEqual(results[0].status, "skipped")
        self.assertIn("SUP_SKILLS_UPDATE", results[0].reason)

    def test_present_required_env_allows_optional_job_to_run(self):
        calls = []
        env = {"SUP_SKILLS_UPDATE": "/tmp/update-skills"}
        jobs = [
            job
            for job in load_jobs_config(config_path(), env=env).jobs
            if job.name == "skills"
        ]
        runner = Runner(
            home=Path("/tmp/example-home"),
            command_exists=lambda name: True,
            path_exists=lambda path: True,
            env=env,
            command_runner=lambda job: (
                calls.append(job.command) or CommandResult(exit_code=0)
            ),
        )

        results = runner.run(jobs, dry_run=False)

        self.assertEqual(results[0].status, "succeeded")
        self.assertEqual(calls, [("/tmp/update-skills",)])

    def test_sudo_preflight_failure_skips_optional_pnpm_command(self):
        calls = []
        jobs = [
            job for job in load_jobs_config(config_path()).jobs if job.name == "pnpm"
        ]
        runner = Runner(
            home=Path("/tmp/example-home"),
            command_exists=lambda name: True,
            path_exists=lambda path: True,
            sudo_preflight=lambda: False,
            command_runner=lambda job: (
                calls.append(job.name) or CommandResult(exit_code=0)
            ),
        )

        results = runner.run(jobs, dry_run=False)

        self.assertEqual(calls, [])
        self.assertEqual(results[0].status, "skipped")
        self.assertIn("sudo authentication", results[0].reason)
        self.assertEqual(Runner.exit_code_for(results), 0)

    def test_sudo_preflight_uses_non_interactive_validation(self):
        runner = Runner(home=Path("/tmp/example-home"))

        with patch("sup.runner.subprocess.run") as run:
            run.return_value.returncode = 0

            self.assertTrue(runner._sudo_preflight())

        run.assert_called_once_with(["sudo", "-n", "-v"], check=False)

    def test_missing_cargo_install_update_subcommand_is_skipped(self):
        jobs = [
            job
            for job in load_jobs_config(config_path()).jobs
            if job.name == "cargo-install-update"
        ]
        runner = Runner(
            home=Path("/tmp/example-home"),
            command_exists=lambda name: name == "cargo",
            path_exists=lambda path: True,
        )

        results = runner.run(jobs, dry_run=False)

        self.assertEqual(results[0].status, "skipped")
        self.assertIn("cargo-install-update", results[0].reason)

    def test_runner_emits_status_updates(self):
        events = []

        def fake_runner(job):
            return CommandResult(exit_code=0)

        jobs = [
            job for job in load_jobs_config(config_path()).jobs if job.name == "rustup"
        ]
        runner = Runner(
            home=Path("/tmp/example-home"),
            command_exists=lambda name: True,
            path_exists=lambda path: True,
            command_runner=fake_runner,
        )

        runner.run(
            jobs,
            dry_run=False,
            on_update=lambda name, status, result=None: events.append((name, status)),
        )

        self.assertEqual(events, [("rustup", "running"), ("rustup", "succeeded")])

    def test_failed_job_makes_runner_unsuccessful(self):
        def fake_runner(job):
            return CommandResult(exit_code=7)

        jobs = [
            job for job in load_jobs_config(config_path()).jobs if job.name == "rustup"
        ]
        runner = Runner(
            home=Path("/tmp/example-home"),
            command_exists=lambda name: True,
            path_exists=lambda path: True,
            command_runner=fake_runner,
        )

        results = runner.run(jobs, dry_run=False)

        self.assertEqual(results[0].status, "failed")
        self.assertEqual(Runner.exit_code_for(results), 1)


class CliTest(unittest.TestCase):
    def test_list_prints_job_names(self):
        out = io.StringIO()

        with redirect_stdout(out):
            code = main(["--list"])

        self.assertEqual(code, 0)
        self.assertIn("brew-upgrade", out.getvalue())
        self.assertIn("skills", out.getvalue())


class DisplayTest(unittest.TestCase):
    def test_dry_run_renders_mission_control_theme(self):
        jobs = load_jobs_config(config_path()).jobs[:2]
        console = Console(file=io.StringIO(), force_terminal=True, width=120)

        render_dry_run(jobs, console=console)
        output = console.file.getvalue()

        self.assertIn("ORBITAL PREFLIGHT", output)
        self.assertIn("TokyoNight", output)
        self.assertIn("🚀", output)
        self.assertIn("█", output)
        self.assertIn("#7aa2f7", output)

    def test_live_dashboard_renders_status_emoji_and_progress(self):
        jobs = [
            job for job in load_jobs_config(config_path()).jobs if job.name == "rustup"
        ]
        console = Console(file=io.StringIO(), force_terminal=True, width=120)
        dashboard = LiveDashboard(jobs, console=console)

        dashboard.update("rustup", "running")
        renderable = dashboard.render()
        console.print(renderable)
        output = console.file.getvalue()

        self.assertIn("🛰️", output)
        self.assertIn("running", output)
        self.assertIn("░", output)
        self.assertIn("rustup", output)
        self.assertIn("sup", output)
        self.assertNotIn("TOKYONIGHT MISSION CONTROL", output)
        self.assertNotIn("SUP ORBITAL COMMAND", output)

    def test_running_progress_bar_animates_by_frame(self):
        first = progress_bar("running", width=12, frame=0).plain
        second = progress_bar("running", width=12, frame=1).plain

        self.assertNotEqual(first, second)
        self.assertEqual(len(first), 12)
        self.assertEqual(len(second), 12)

    def test_non_running_progress_bars_do_not_animate(self):
        for status in ("queued", "succeeded", "failed", "skipped"):
            with self.subTest(status=status):
                self.assertEqual(
                    progress_bar(status, width=12, frame=0).plain,
                    progress_bar(status, width=12, frame=5).plain,
                )

    def test_summary_renders_grouped_mission_report(self):
        job = [
            job for job in load_jobs_config(config_path()).jobs if job.name == "rustup"
        ][0]
        result = JobResult(
            job=job,
            status="succeeded",
            exit_code=0,
            elapsed=1.25,
            reason="",
        )
        console = Console(file=io.StringIO(), force_terminal=True, width=120)

        render_summary([result], console=console, tail_count=40)
        output = console.file.getvalue()

        self.assertIn("MISSION REPORT", output)
        self.assertIn("✅", output)
        self.assertIn("succeeded", output)
        self.assertIn("rustup", output)


if __name__ == "__main__":
    unittest.main()
