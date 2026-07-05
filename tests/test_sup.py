import io
import re
import signal
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest.mock import patch

from rich.console import Console

from sup.cli import (
    INTERRUPTED_EXIT_CODE,
    SudoAuthenticator,
    authenticate_sudo_with_overlay,
    has_sudo_ticket,
    raise_keyboard_interrupt,
    main,
    validate_sudo_password,
)
from sup.display import (
    TOKYONIGHT,
    LiveDashboard,
    display_command,
    progress_bar,
    render_dry_run,
    render_summary,
)
from sup.jobs import Job, config_path, load_jobs_config, resolve_job_selection
from sup.logs import cleanup_old_runs, create_run_dir, tail_lines
from sup.runner import CommandResult, JobResult, Runner


ANSI_STYLE = re.compile(r"\x1b\[[0-9;]*m")


def find_line_index(lines: list[str], needle: str) -> int:
    for index, line in enumerate(lines):
        if needle in line:
            return index
    raise AssertionError(f"{needle!r} not found")


def strip_ansi_styles(value: str) -> str:
    return ANSI_STYLE.sub("", value)


def terminal_console(*, width: int = 120) -> Console:
    return Console(
        file=io.StringIO(),
        force_terminal=True,
        width=width,
        _environ={},
    )


class SelectionTest(unittest.TestCase):
    def test_default_jobs_preserve_core_then_parallel_order(self):
        jobs_config = load_jobs_config(config_path())
        jobs = jobs_config.jobs

        self.assertEqual(
            [job.name for job in jobs if job.phase == "core"],
            [
                "brew-link-node-tools",
                "brew-upgrade",
                "brew-relink-node-tools",
                "brew-cleanup",
                "zimfw-upgrade",
            ],
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

    def test_brew_node_tool_links_are_repaired_around_upgrade(self):
        jobs_config = load_jobs_config(config_path())
        jobs = {job.name: job for job in jobs_config.jobs}
        expected_command = (
            "sh",
            "-c",
            'for formula in pnpm node; do if brew list --formula "$formula" '
            '>/dev/null 2>&1; then brew link --overwrite "$formula"; fi; done',
        )

        for name in ("brew-link-node-tools", "brew-relink-node-tools"):
            with self.subTest(name=name):
                job = jobs[name]
                self.assertEqual(job.command, expected_command)
                self.assertEqual(job.required_commands, ("brew", "sh"))
                self.assertTrue(job.optional)

    def test_brew_upgrade_preflights_sudo_for_cask_scripts(self):
        jobs_config = load_jobs_config(config_path())
        brew_upgrade = next(
            job for job in jobs_config.jobs if job.name == "brew-upgrade"
        )

        self.assertTrue(brew_upgrade.sudo_preflight)
        self.assertIn("sudo", brew_upgrade.required_commands)

    def test_mas_preflights_sudo_for_update_subprocesses(self):
        jobs_config = load_jobs_config(config_path())
        mas = next(job for job in jobs_config.jobs if job.name == "mas")

        self.assertTrue(mas.sudo_preflight)
        self.assertIn("sudo", mas.required_commands)

    def test_pnpm_updates_globals_without_sudo(self):
        home = Path("/tmp/example-home")
        jobs_config = load_jobs_config(
            config_path(),
            home=home,
            env={"PATH": "/opt/homebrew/bin:/usr/bin"},
        )
        pnpm = next(job for job in jobs_config.jobs if job.name == "pnpm")

        self.assertEqual(
            pnpm.command,
            (
                "pnpm",
                "update",
                "--global",
            ),
        )
        self.assertFalse(pnpm.sudo_preflight)
        self.assertNotIn("sudo", pnpm.required_commands)
        self.assertEqual(pnpm.required_paths, ())

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

    def test_missing_optional_path_is_skipped(self):
        jobs = [
            job
            for job in load_jobs_config(config_path(), env={}).jobs
            if job.name == "skills"
        ]
        runner = Runner(
            home=Path("/tmp/example-home"),
            command_exists=lambda name: True,
            path_exists=lambda path: False,
            env={},
            command_runner=lambda job: CommandResult(exit_code=0),
        )

        results = runner.run(jobs, dry_run=False)

        self.assertEqual(results[0].status, "skipped")
        self.assertIn(".agents/skills/update.py", results[0].reason)

    def test_skills_job_uses_agents_update_script(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            updater = home / ".agents" / "skills" / "update.py"
            jobs = [
                job
                for job in load_jobs_config(config_path(), home=home).jobs
                if job.name == "skills"
            ]

        self.assertEqual(jobs[0].command, ("python3", str(updater)))
        self.assertEqual(jobs[0].required_commands, ("python3",))
        self.assertEqual(jobs[0].required_paths, (updater,))
        self.assertEqual(jobs[0].required_env, ())

    def test_present_skills_updater_allows_optional_job_to_run(self):
        calls = []
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            jobs = [
                job
                for job in load_jobs_config(config_path(), home=home).jobs
                if job.name == "skills"
            ]
            runner = Runner(
                home=Path("/tmp/example-home"),
                command_exists=lambda name: True,
                path_exists=lambda path: True,
                command_runner=lambda job: (
                    calls.append(job.command) or CommandResult(exit_code=0)
                ),
            )

            results = runner.run(jobs, dry_run=False)

        self.assertEqual(results[0].status, "succeeded")
        self.assertEqual(
            calls,
            [("python3", str(home / ".agents" / "skills" / "update.py"))],
        )

    def test_sudo_preflight_failure_skips_optional_mas_command(self):
        calls = []
        jobs = [
            job for job in load_jobs_config(config_path()).jobs if job.name == "mas"
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

    def test_sudo_preflight_uses_sup_password_prompt(self):
        runner = Runner(home=Path("/tmp/example-home"))

        with patch("sup.runner.subprocess.run") as run:
            run.return_value.returncode = 0

            self.assertTrue(runner._sudo_preflight())

        run.assert_called_once_with(
            ["sudo", "-p", "sup sudo password: ", "-v"],
            check=False,
        )

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

    def test_runner_emits_subprocess_output_updates(self):
        events = []
        job = Job(
            name="example",
            label="Example",
            phase="core",
            command=(
                sys.executable,
                "-c",
                "print('step one'); print('step two')",
            ),
            required_commands=(),
            optional=True,
            log_name="example.log",
        )

        with tempfile.TemporaryDirectory() as tmp:
            runner = Runner(
                home=Path(tmp),
                command_exists=lambda name: True,
                path_exists=lambda path: True,
            )

            results = runner.run(
                [job],
                dry_run=False,
                on_update=lambda name, status, result=None, output=None: events.append(
                    (name, status, output)
                ),
            )

        self.assertEqual(results[0].status, "succeeded")
        self.assertIn(("example", "running", "step one"), events)
        self.assertIn(("example", "running", "step two"), events)

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

    def test_stop_terminates_active_subprocesses(self):
        class FakeProcess:
            def __init__(self):
                self.terminated = False
                self.killed = False
                self.waited = False

            def poll(self):
                return None

            def terminate(self):
                self.terminated = True

            def wait(self, timeout=None):
                self.waited = True
                return -15

            def kill(self):
                self.killed = True

        process = FakeProcess()
        runner = Runner(home=Path("/tmp/example-home"))
        runner._track_process(process)

        runner.stop()

        self.assertTrue(process.terminated)
        self.assertTrue(process.waited)
        self.assertFalse(process.killed)

    def test_parallel_interrupt_requests_runner_stop(self):
        jobs = [
            job for job in load_jobs_config(config_path()).jobs if job.name == "rustup"
        ]
        runner = Runner(
            home=Path("/tmp/example-home"),
            command_exists=lambda name: True,
            path_exists=lambda path: True,
            command_runner=lambda job: (_ for _ in ()).throw(KeyboardInterrupt),
        )

        with patch.object(runner, "stop") as stop:
            with self.assertRaises(KeyboardInterrupt):
                runner.run(jobs, dry_run=False)

        stop.assert_called_once_with()


class CliTest(unittest.TestCase):
    def test_list_prints_job_names(self):
        out = io.StringIO()

        with redirect_stdout(out):
            code = main(["--list"])

        self.assertEqual(code, 0)
        self.assertIn("brew-upgrade", out.getvalue())
        self.assertIn("skills", out.getvalue())

    def test_sudo_overlay_reads_password_and_clears(self):
        events = []
        job = Job(
            name="brew-upgrade",
            label="Homebrew upgrade",
            phase="core",
            command=("brew", "upgrade"),
            required_commands=("brew", "sudo"),
            optional=False,
            log_name="brew-upgrade.log",
            sudo_preflight=True,
        )

        class FakeDashboard:
            def show_auth_overlay(self, jobs, *, error=None):
                events.append(("show", tuple(job.name for job in jobs), error))

            def clear_auth_overlay(self):
                events.append(("clear",))

        ticket_checks = iter([False, True])
        ok = authenticate_sudo_with_overlay(
            [job],
            dashboard=FakeDashboard(),
            sudo_ticket_available=lambda: next(ticket_checks),
            password_reader=lambda: "secret",
            validator=lambda password: password == "secret",
        )

        self.assertTrue(ok)
        self.assertEqual(
            events,
            [
                ("show", ("brew-upgrade",), None),
                ("clear",),
            ],
        )

    def test_sudo_overlay_confirms_ticket_after_validation(self):
        job = Job(
            name="pnpm",
            label="pnpm globals",
            phase="parallel",
            command=("sudo", "-n", "pnpm", "update", "--global"),
            required_commands=("sudo", "pnpm"),
            optional=True,
            log_name="pnpm.log",
            sudo_preflight=True,
        )
        ticket_checks = iter([False, False])

        class FakeDashboard:
            console = Console(file=io.StringIO())

            def show_auth_overlay(self, jobs, *, error=None):
                pass

            def clear_auth_overlay(self):
                pass

        ok = authenticate_sudo_with_overlay(
            [job],
            dashboard=FakeDashboard(),
            sudo_ticket_available=lambda: next(ticket_checks),
            password_reader=lambda: "secret",
            validator=lambda password: True,
            max_attempts=1,
        )

        self.assertFalse(ok)

    def test_validate_sudo_password_uses_stdin_promptless_sudo(self):
        with patch("sup.cli.subprocess.run") as run:
            run.return_value.returncode = 0

            self.assertTrue(validate_sudo_password("secret"))

        run.assert_called_once_with(
            ["sudo", "-S", "-p", "", "-v"],
            input="secret\n",
            text=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )

    def test_has_sudo_ticket_refreshes_real_noninteractive_ticket(self):
        with patch("sup.cli.subprocess.run") as run:
            run.return_value.returncode = 0

            self.assertTrue(has_sudo_ticket())

        run.assert_called_once_with(
            ["sudo", "-n", "-v"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )

    def test_sudo_authenticator_prompts_again_after_ticket_expires(self):
        events = []
        job = Job(
            name="pnpm",
            label="pnpm globals",
            phase="parallel",
            command=("sudo", "-n", "pnpm", "update", "--global"),
            required_commands=("sudo", "pnpm"),
            optional=True,
            log_name="pnpm.log",
            sudo_preflight=True,
        )

        class FakeDashboard:
            console = Console(file=io.StringIO())

            def show_auth_overlay(self, jobs, *, error=None):
                events.append(("show", tuple(job.name for job in jobs), error))

            def clear_auth_overlay(self):
                events.append(("clear",))

        ticket_checks = iter([False, True, False, True])
        passwords = iter(["first", "second"])
        authenticator = SudoAuthenticator(
            [job],
            dashboard=FakeDashboard(),
            sudo_ticket_available=lambda: next(ticket_checks),
            password_reader=lambda: next(passwords),
            validator=lambda password: password in {"first", "second"},
        )

        self.assertTrue(authenticator.authenticate())
        self.assertTrue(authenticator.authenticate())
        self.assertEqual(
            events,
            [
                ("show", ("pnpm",), None),
                ("clear",),
                ("show", ("pnpm",), None),
                ("clear",),
            ],
        )

    def test_main_reuses_prompting_sudo_callback_during_run(self):
        out = io.StringIO()
        with (
            patch("sup.cli.Runner") as runner_class,
            patch("sup.cli.SudoAuthenticator") as authenticator_class,
            redirect_stdout(out),
        ):
            runner = runner_class.return_value
            authenticator = authenticator_class.return_value
            authenticator.authenticate.side_effect = [True, True, True]
            runner_class.exit_code_for.return_value = 0

            def run_jobs(_jobs, *, on_update):
                self.assertTrue(runner.sudo_preflight())
                self.assertTrue(runner.sudo_preflight())
                return []

            runner.run.side_effect = run_jobs

            code = main(["--only", "pnpm"])

        self.assertEqual(code, 0)
        self.assertEqual(authenticator.authenticate.call_count, 3)

    def test_keyboard_interrupt_stops_runner_and_returns_130(self):
        out = io.StringIO()

        with (
            patch("sup.cli.Runner") as runner_class,
            redirect_stdout(out),
        ):
            runner = runner_class.return_value
            runner.run.side_effect = KeyboardInterrupt

            code = main(["--only", "rustup"])

        self.assertEqual(code, INTERRUPTED_EXIT_CODE)
        runner.stop.assert_called_once_with()
        self.assertIn("Run interrupted", out.getvalue())
        self.assertNotIn("Traceback", out.getvalue())

    def test_sigterm_uses_keyboard_interrupt_path(self):
        with self.assertRaises(KeyboardInterrupt):
            raise_keyboard_interrupt(signal.SIGTERM, None)


class DisplayTest(unittest.TestCase):
    def render_dashboard_lines(
        self,
        dashboard: LiveDashboard,
        *,
        width: int = 160,
    ) -> list[str]:
        console = terminal_console(width=width)
        console.print(dashboard.render())
        return console.file.getvalue().splitlines()

    def test_dry_run_renders_mission_control_theme(self):
        jobs = load_jobs_config(config_path()).jobs[:2]
        console = terminal_console(width=120)

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
        console = terminal_console(width=120)
        dashboard = LiveDashboard(jobs, console=console)

        dashboard.update("rustup", "running", output="older preface")
        dashboard.update("rustup", "running", output="syncing channel")
        dashboard.update("rustup", "running", output="downloading rustc")
        dashboard.update("rustup", "running", output="installing rustc")
        renderable = dashboard.render()
        console.print(renderable)
        output = console.file.getvalue()

        self.assertIn("🛰️", output)
        self.assertIn("running", output)
        self.assertIn("░", output)
        self.assertIn("rustup", output)
        self.assertNotIn("older preface", output)
        self.assertIn("syncing channel", output)
        self.assertIn("downloading rustc", output)
        self.assertIn("installing rustc", output)
        self.assertNotIn("TOKYONIGHT MISSION CONTROL", output)
        self.assertNotIn("SUP ORBITAL COMMAND", output)

    def test_display_command_shortens_long_env_assignments(self):
        command = display_command(
            (
                "env",
                "PATH=/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/local/sbin:/usr/bin:/bin:~/Library/pnpm",
                "example",
            )
        )

        self.assertIn(
            "PATH=/opt/homebrew/bin:/opt/homebrew/sbin:…:~/Library/pnpm", command
        )
        self.assertNotIn("/usr/local/bin:/usr/local/sbin", command)

    def test_live_dashboard_omits_duplicate_sup_chrome(self):
        jobs = [
            Job(
                name="example",
                label="Example",
                phase="core",
                command=("example", "update"),
                required_commands=(),
                optional=True,
                log_name="example.log",
            )
        ]
        console = terminal_console(width=120)
        dashboard = LiveDashboard(jobs, console=console)

        renderable = dashboard.render()
        console.print(renderable)
        output = console.file.getvalue()

        self.assertIsNone(renderable.title)
        self.assertNotIn("sup", output)

    def test_live_dashboard_renders_sudo_auth_overlay(self):
        jobs = [
            Job(
                name="brew-upgrade",
                label="Homebrew upgrade",
                phase="core",
                command=("brew", "upgrade"),
                required_commands=("brew", "sudo"),
                optional=False,
                log_name="brew-upgrade.log",
                sudo_preflight=True,
            )
        ]
        console = terminal_console(width=120)
        dashboard = LiveDashboard(jobs, console=console)

        dashboard.show_auth_overlay(jobs)
        console.print(dashboard.render())
        output = console.file.getvalue()

        self.assertIn("sudo authentication required", output)
        self.assertIn("brew-upgrade", output)
        self.assertIn("Password", output)
        self.assertIn("hidden input active", output)
        self.assertRegex(output, r"\x1b\[[0-9;]*2[0-9;]*m")

    def test_sudo_auth_overlay_does_not_reflow_dashboard(self):
        jobs = load_jobs_config(config_path()).jobs
        base_dashboard = LiveDashboard(
            jobs,
            console=terminal_console(width=160),
        )
        overlay_dashboard = LiveDashboard(
            jobs,
            console=terminal_console(width=160),
        )
        overlay_dashboard.show_auth_overlay([job for job in jobs if job.sudo_preflight])

        base_lines = self.render_dashboard_lines(base_dashboard, width=160)
        overlay_lines = self.render_dashboard_lines(overlay_dashboard, width=160)

        self.assertEqual(len(overlay_lines), len(base_lines))
        self.assertEqual(
            find_line_index(base_lines, "npm"),
            find_line_index(overlay_lines, "npm"),
        )
        self.assertEqual(
            find_line_index(base_lines, "skills"),
            find_line_index(overlay_lines, "skills"),
        )
        prompt_line = strip_ansi_styles(
            overlay_lines[
                find_line_index(overlay_lines, "sudo authentication required")
            ]
        )
        prompt_start = prompt_line.index("sudo authentication required")
        self.assertGreater(prompt_start, 45)
        self.assertLess(prompt_start, 90)

    def test_live_dashboard_refreshes_only_on_updates(self):
        jobs = [
            Job(
                name="example",
                label="Example",
                phase="core",
                command=("example", "update"),
                required_commands=(),
                optional=True,
                log_name="example.log",
            )
        ]
        console = terminal_console(width=120)
        dashboard = LiveDashboard(jobs, console=console)

        with patch("sup.display.Live") as live:
            with dashboard:
                dashboard.update("example", "running")

        live.assert_called_once_with(dashboard, console=console, auto_refresh=False)
        live.return_value.update.assert_called_once_with(dashboard, refresh=True)

    def test_live_dashboard_throttles_output_refreshes(self):
        jobs = [
            Job(
                name="example",
                label="Example",
                phase="core",
                command=("example", "update"),
                required_commands=(),
                optional=True,
                log_name="example.log",
            )
        ]
        console = terminal_console(width=120)
        now = 0.0
        dashboard = LiveDashboard(jobs, console=console, clock=lambda: now)

        with patch("sup.display.Live") as live:
            with dashboard:
                dashboard.update("example", "running")
                dashboard.update("example", "running", output="first line")
                dashboard.update("example", "running", output="second line")

        refresh_values = [
            call.kwargs["refresh"] for call in live.return_value.update.call_args_list
        ]
        self.assertEqual(refresh_values, [True, True, False])

    def test_muted_palette_is_readable_on_dim_backgrounds(self):
        self.assertEqual(TOKYONIGHT["muted"], "#8f97c7")

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
        console = terminal_console(width=120)

        render_summary([result], console=console, tail_count=40)
        output = console.file.getvalue()

        self.assertIn("MISSION REPORT", output)
        self.assertIn("✅", output)
        self.assertIn("succeeded", output)
        self.assertIn("rustup", output)


if __name__ == "__main__":
    unittest.main()
