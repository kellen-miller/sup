import io
import re
import signal
import subprocess
import sys
import tempfile
import threading
import unittest
from contextlib import redirect_stdout
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest.mock import patch

from rich.cells import cell_len
from rich.console import Console, ConsoleDimensions

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
from tests.terminal_harness import analyze


ANSI_STYLE = re.compile(r"\x1b\[[0-9;]*m")


def find_line_index(lines: list[str], needle: str) -> int:
    for index, line in enumerate(lines):
        if needle in line:
            return index
    raise AssertionError(f"{needle!r} not found")


def strip_ansi_styles(value: str) -> str:
    return ANSI_STYLE.sub("", value)


def terminal_console(*, width: int = 120, height: int = 25) -> Console:
    return Console(
        file=io.StringIO(),
        force_terminal=True,
        width=width,
        height=height,
        _environ={},
    )


class MutableSizeConsole(Console):
    def __init__(self, *, width: int, height: int) -> None:
        self.dimensions = ConsoleDimensions(width, height)
        super().__init__(
            file=io.StringIO(),
            force_terminal=True,
            _environ={},
        )

    @property
    def size(self) -> ConsoleDimensions:
        return self.dimensions

    def set_size(self, *, width: int, height: int) -> None:
        self.dimensions = ConsoleDimensions(width, height)


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
                "node-package-managers",
                "rustup",
                "cargo-install-update",
                "skills",
            ],
        )

    def test_brew_node_link_is_repaired_around_upgrade(self):
        jobs_config = load_jobs_config(config_path())
        jobs = {job.name: job for job in jobs_config.jobs}
        expected_command = (
            "sh",
            "-c",
            "if brew list --formula node >/dev/null 2>&1; then "
            "brew link --overwrite node; fi",
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

    def test_zimfw_jobs_source_zim_init_with_zsh(self):
        home = Path("/tmp/example-home")
        jobs_config = load_jobs_config(config_path(), home=home)
        jobs = {job.name: job for job in jobs_config.jobs}

        self.assertEqual(
            jobs["zimfw-upgrade"].command,
            (
                "zsh",
                "-lc",
                f'source "{home}/.zim/init.zsh"; zimfw upgrade',
            ),
        )
        self.assertEqual(jobs["zimfw-upgrade"].required_commands, ("zsh",))
        self.assertEqual(
            jobs["zimfw-upgrade"].required_paths,
            (home / ".zim" / "init.zsh",),
        )
        self.assertEqual(
            jobs["zimfw-update"].command,
            (
                "zsh",
                "-lc",
                f'source "{home}/.zim/init.zsh"; zimfw update',
            ),
        )
        self.assertEqual(jobs["zimfw-update"].required_commands, ("zsh",))
        self.assertEqual(
            jobs["zimfw-update"].required_paths,
            (home / ".zim" / "init.zsh",),
        )

    def test_mas_preflights_sudo_for_update_subprocesses(self):
        jobs_config = load_jobs_config(config_path())
        mas = next(job for job in jobs_config.jobs if job.name == "mas")

        self.assertTrue(mas.sudo_preflight)
        self.assertIn("sudo", mas.required_commands)

    def test_node_package_managers_update_in_order_without_sudo(self):
        jobs_config = load_jobs_config(config_path())
        jobs = {job.name: job for job in jobs_config.jobs}
        self.assertIn("node-package-managers", jobs)
        job = jobs["node-package-managers"]

        self.assertEqual(
            job.command,
            (
                "sh",
                "-c",
                "npm update --global && "
                "corepack install --global pnpm@latest && "
                "pnpm update --global",
            ),
        )
        self.assertEqual(job.required_commands, ("sh", "npm", "corepack", "pnpm"))
        self.assertFalse(job.sudo_preflight)
        self.assertNotIn("sudo", job.required_commands)

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
        self.assertIn("node-package-managers", names)

        for selector in ("npm", "pnpm", "node-package-managers"):
            with self.subTest(selector=selector):
                selected = resolve_job_selection(
                    jobs,
                    aliases=jobs_config.aliases,
                    only=[selector],
                    skip=[],
                )
                self.assertEqual(
                    [job.name for job in selected], ["node-package-managers"]
                )

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
            sudo_preflight=lambda job: False,
            command_runner=lambda job: (
                calls.append(job.name) or CommandResult(exit_code=0)
            ),
        )

        results = runner.run(jobs, dry_run=False)

        self.assertEqual(calls, [])
        self.assertEqual(results[0].status, "skipped")
        self.assertIn("sudo authentication", results[0].reason)
        self.assertEqual(Runner.exit_code_for(results), 0)

    def test_sudo_preflight_receives_current_job(self):
        preflight_jobs = []
        jobs = [
            job for job in load_jobs_config(config_path()).jobs if job.name == "mas"
        ]
        runner = Runner(
            home=Path("/tmp/example-home"),
            command_exists=lambda name: True,
            path_exists=lambda path: True,
            sudo_preflight=lambda job: preflight_jobs.append(job.name) or True,
            command_runner=lambda job: CommandResult(exit_code=0),
        )

        results = runner.run(jobs, dry_run=False)

        self.assertEqual(results[0].status, "succeeded")
        self.assertEqual(preflight_jobs, ["mas"])

    def test_parallel_jobs_prepare_on_main_before_worker_execution(self):
        main_thread = threading.get_ident()
        events = []
        event_lock = threading.Lock()
        command_barrier = threading.Barrier(3)

        def record(event):
            with event_lock:
                events.append((event, threading.get_ident()))

        def command_exists(name):
            record(f"requirement:{name}")
            return True

        def sudo_preflight(job):
            record(f"preflight:{job.name}")
            return True

        def command_runner(job):
            record(f"command:{job.name}")
            command_barrier.wait(timeout=2)
            return CommandResult(exit_code=0)

        jobs = [
            Job(
                name=name,
                label=name.title(),
                phase="parallel",
                command=(name,),
                required_commands=(name,),
                optional=False,
                log_name=f"{name}.log",
                sudo_preflight=name != "plain",
            )
            for name in ("first", "plain", "last")
        ]

        with tempfile.TemporaryDirectory() as tmp:
            results = Runner(
                home=Path(tmp),
                command_exists=command_exists,
                sudo_preflight=sudo_preflight,
                command_runner=command_runner,
            ).run(jobs)

        first_command = next(
            index
            for index, (event, _thread) in enumerate(events)
            if event.startswith("command:")
        )
        preparation = events[:first_command]
        commands = events[first_command:]
        self.assertEqual(
            [event for event, _thread in preparation],
            [
                "requirement:first",
                "preflight:first",
                "requirement:plain",
                "requirement:last",
                "preflight:last",
            ],
        )
        self.assertTrue(all(thread == main_thread for _event, thread in preparation))
        self.assertEqual({thread for _event, thread in commands} & {main_thread}, set())
        self.assertEqual(len({thread for _event, thread in commands}), 3)
        self.assertEqual(
            [result.job.name for result in results], [job.name for job in jobs]
        )

    def test_parallel_preflight_failure_preserves_result_order(self):
        jobs = [
            Job(
                name=name,
                label=name.title(),
                phase="parallel",
                command=(name,),
                required_commands=(),
                optional=name == "blocked",
                log_name=f"{name}.log",
                sudo_preflight=name == "blocked",
            )
            for name in ("ready-before", "blocked", "ready-after")
        ]

        with tempfile.TemporaryDirectory() as tmp:
            results = Runner(
                home=Path(tmp),
                sudo_preflight=lambda job: False,
                command_runner=lambda job: CommandResult(exit_code=0),
            ).run(jobs)

        self.assertEqual(
            [(result.job.name, result.status) for result in results],
            [
                ("ready-before", "succeeded"),
                ("blocked", "skipped"),
                ("ready-after", "succeeded"),
            ],
        )

    def test_parallel_elapsed_excludes_serial_preflight_time(self):
        now = [0.0]
        job = Job(
            name="ready",
            label="Ready",
            phase="parallel",
            command=("ready",),
            required_commands=(),
            optional=False,
            log_name="ready.log",
            sudo_preflight=True,
        )

        def sudo_preflight(_job):
            now[0] = 100.0
            return True

        def command_runner(_job):
            now[0] = 101.0
            return CommandResult(exit_code=0)

        with tempfile.TemporaryDirectory() as tmp:
            with patch("sup.runner.time.monotonic", side_effect=lambda: now[0]):
                result = Runner(
                    home=Path(tmp),
                    sudo_preflight=sudo_preflight,
                    command_runner=command_runner,
                ).run([job])[0]

        self.assertEqual(result.elapsed, 1.0)

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
            def read_password(self, jobs, *, error=None, reader=None):
                events.append(("read", tuple(job.name for job in jobs), error))
                return reader()

        passwords = iter(["wrong", "secret"])
        ok = authenticate_sudo_with_overlay(
            [job],
            dashboard=FakeDashboard(),
            sudo_ticket_available=lambda: False,
            password_reader=lambda: next(passwords),
            validator=lambda password: password == "secret",
        )

        self.assertTrue(ok)
        self.assertEqual(
            events,
            [
                ("read", ("brew-upgrade",), None),
                (
                    "read",
                    ("brew-upgrade",),
                    "Authentication failed. Try again.",
                ),
            ],
        )

    def test_sudo_overlay_trusts_successful_validation(self):
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
        ticket_checks = iter([False])

        class FakeDashboard:
            def read_password(self, jobs, *, error=None, reader=None):
                return reader()

        ok = authenticate_sudo_with_overlay(
            [job],
            dashboard=FakeDashboard(),
            sudo_ticket_available=lambda: next(ticket_checks),
            password_reader=lambda: "secret",
            validator=lambda password: True,
            max_attempts=1,
        )

        self.assertTrue(ok)

    def test_sudo_overlay_treats_end_of_input_as_unavailable(self):
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
            def read_password(self, jobs, *, error=None, reader=None):
                raise EOFError

        self.assertFalse(
            authenticate_sudo_with_overlay(
                [job],
                dashboard=FakeDashboard(),
                sudo_ticket_available=lambda: False,
            )
        )

    def test_sudo_overlay_treats_invisible_prompt_as_unavailable(self):
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
        dashboard = LiveDashboard(
            [job],
            console=terminal_console(width=80, height=6),
        )

        self.assertFalse(
            authenticate_sudo_with_overlay(
                [job],
                dashboard=dashboard,
                sudo_ticket_available=lambda: False,
                validator=lambda _password: self.fail("validator should not run"),
            )
        )
        lines = dashboard.console.render_lines(
            dashboard.render(),
            dashboard.console.options,
            pad=False,
        )
        rendered = "\n".join(
            "".join(segment.text for segment in line) for line in lines
        )
        self.assertNotIn("Password:", rendered)

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

    def test_sudo_authenticator_reuses_valid_ticket_without_prompt(self):
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
            def read_password(self, jobs, *, error=None, reader=None):
                events.append(("read", tuple(job.name for job in jobs), error))
                return reader()

        ticket_checks = iter([False, True])
        passwords = iter(["first"])
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
                ("read", ("pnpm",), None),
            ],
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
            def read_password(self, jobs, *, error=None, reader=None):
                events.append(("read", tuple(job.name for job in jobs), error))
                return reader()

        ticket_checks = iter([False, False])
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
                ("read", ("pnpm",), None),
                ("read", ("pnpm",), None),
            ],
        )

    def test_main_uses_current_job_for_sudo_prompting(self):
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

            def run_jobs(jobs, *, on_update):
                sudo_jobs = {job.name: job for job in jobs if job.sudo_preflight}
                self.assertTrue(runner.sudo_preflight(sudo_jobs["brew-upgrade"]))
                self.assertTrue(runner.sudo_preflight(sudo_jobs["mas"]))
                return []

            runner.run.side_effect = run_jobs

            code = main(["--only", "brew-upgrade", "--only", "mas"])

        self.assertEqual(code, 0)
        self.assertEqual(authenticator.authenticate.call_count, 2)
        self.assertEqual(
            [call.args[0].name for call in authenticator.authenticate.call_args_list],
            ["brew-upgrade", "mas"],
        )

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
    def rendered_height(self, dashboard: LiveDashboard) -> int:
        console = dashboard.console
        return len(console.render_lines(dashboard.render(), console.options, pad=False))

    def rendered_text(self, dashboard: LiveDashboard) -> str:
        console = dashboard.console
        lines = console.render_lines(dashboard.render(), console.options, pad=False)
        return "\n".join("".join(segment.text for segment in line) for line in lines)

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

    def test_live_dashboard_budgets_default_jobs_to_viewport_height(self):
        jobs = load_jobs_config(config_path()).jobs

        for width, height in ((80, 18), (120, 18), (120, 24), (120, 36)):
            with self.subTest(width=width, height=height):
                dashboard = LiveDashboard(
                    jobs,
                    console=terminal_console(width=width, height=height),
                )

                self.assertLessEqual(self.rendered_height(dashboard), height)

    def test_live_dashboard_budgets_narrow_viewports(self):
        jobs = load_jobs_config(config_path()).jobs

        for width, height in (
            (40, 18),
            (30, 12),
            (80, 3),
            (80, 2),
            (80, 1),
        ):
            with self.subTest(width=width, height=height):
                dashboard = LiveDashboard(
                    jobs,
                    console=terminal_console(width=width, height=height),
                )

                self.assertLessEqual(self.rendered_height(dashboard), height)

    def test_live_dashboard_rebudgets_after_terminal_resize(self):
        template = load_jobs_config(config_path()).jobs[0]
        jobs = [
            Job(
                name=f"job-{index:02d}",
                label=f"Job {index:02d}",
                phase=template.phase,
                command=("tool", "update"),
                required_commands=(),
                optional=True,
                log_name=f"job-{index:02d}.log",
            )
            for index in range(20)
        ]
        console = MutableSizeConsole(width=120, height=24)
        dashboard = LiveDashboard(jobs, console=console)

        self.assertNotIn("jobs hidden", self.rendered_text(dashboard))
        console.set_size(width=80, height=18)

        self.assertLessEqual(self.rendered_height(dashboard), 18)
        self.assertIn("… 6 jobs hidden", self.rendered_text(dashboard))

    def test_live_dashboard_sorts_active_jobs_first_within_phase(self):
        jobs = [
            job
            for job in load_jobs_config(config_path()).jobs
            if job.name in {"gup", "node-package-managers", "rustup"}
        ]
        console = terminal_console(width=160)
        dashboard = LiveDashboard(jobs, console=console)

        dashboard.update("node-package-managers", "succeeded")
        dashboard.update("rustup", "running")
        console.print(dashboard.render())
        lines = [
            strip_ansi_styles(line) for line in console.file.getvalue().splitlines()
        ]

        self.assertLess(
            find_line_index(lines, "rustup"),
            find_line_index(lines, "gup"),
        )
        self.assertLess(
            find_line_index(lines, "gup"),
            find_line_index(lines, "node-package-managers"),
        )

    def test_live_dashboard_renders_phase_sections(self):
        jobs = [
            job
            for job in load_jobs_config(config_path()).jobs
            if job.name in {"brew-upgrade", "gup"}
        ]
        console = terminal_console(width=160)
        dashboard = LiveDashboard(jobs, console=console)

        console.print(dashboard.render())
        output = strip_ansi_styles(console.file.getvalue())

        self.assertIn("[core]", output)
        self.assertIn("[par]", output)
        self.assertNotIn("core phase", output)
        self.assertNotIn("parallel phase", output)
        self.assertNotIn("brew upgrade", output)

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

        self.assertNotIn("sup", output)
        self.assertNotIn("example update", output)

    def test_live_dashboard_prioritizes_state_and_reports_hidden_jobs(self):
        jobs = load_jobs_config(config_path()).jobs[:7]
        console = terminal_console(width=120, height=8)
        dashboard = LiveDashboard(jobs, console=console)
        statuses = {
            jobs[0].name: "succeeded",
            jobs[1].name: "failed",
            jobs[2].name: "queued",
            jobs[3].name: "skipped",
            jobs[4].name: "running",
            jobs[5].name: "queued",
            jobs[6].name: "succeeded",
        }
        for name, status in statuses.items():
            dashboard.update(name, status)

        console.print(dashboard.render())
        lines = [
            strip_ansi_styles(line) for line in console.file.getvalue().splitlines()
        ]

        self.assertLess(
            find_line_index(lines, jobs[4].name), find_line_index(lines, jobs[1].name)
        )
        self.assertLess(
            find_line_index(lines, jobs[1].name), find_line_index(lines, jobs[2].name)
        )
        self.assertLess(
            find_line_index(lines, jobs[2].name), find_line_index(lines, jobs[5].name)
        )
        self.assertIn("… 3 jobs hidden", "\n".join(lines))
        self.assertNotIn(jobs[0].name, "\n".join(lines))
        self.assertNotIn(jobs[3].name, "\n".join(lines))

    def test_live_dashboard_uses_spare_rows_for_labeled_output_dock(self):
        jobs = load_jobs_config(config_path()).jobs[:2]
        console = terminal_console(width=120, height=10)
        dashboard = LiveDashboard(jobs, console=console)

        dashboard.update(jobs[0].name, "running", output="discarded")
        dashboard.update(jobs[1].name, "running", output="downloading")
        dashboard.update(jobs[0].name, "running", output="linking")
        dashboard.update(jobs[1].name, "running", output="complete")
        console.print(dashboard.render())
        output = strip_ansi_styles(console.file.getvalue())

        self.assertIn("recent output", output)
        self.assertIn(f"{jobs[1].name}: downloading", output)
        self.assertIn(f"{jobs[0].name}: linking", output)
        self.assertIn(f"{jobs[1].name}: complete", output)
        self.assertNotIn("discarded", output)

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

        dashboard.read_password(
            jobs,
            reader=lambda: console.print(dashboard.render()) or "secret",
        )
        output = console.file.getvalue()

        self.assertIn("sudo authentication required", output)
        self.assertIn("brew-upgrade", output)
        self.assertIn("Password", output)
        self.assertRegex(output, r"\x1b\[[0-9;]*2[0-9;]*m")

    def test_live_dashboard_reads_password_at_rendered_prompt_cell(self):
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

        for width, height in ((80, 18), (120, 24)):
            with self.subTest(width=width, height=height):
                console = terminal_console(width=width, height=height)
                dashboard = LiveDashboard(jobs, console=console)
                observed = {}

                def reader():
                    size = console.size
                    lines = console.render_lines(
                        dashboard.render(),
                        console.options.update(width=size.width, height=size.height),
                        pad=False,
                    )
                    observed["lines"] = [
                        "".join(segment.text for segment in line) for line in lines
                    ]
                    observed["controls"] = console.file.getvalue()
                    return "secret"

                password = dashboard.read_password(jobs, reader=reader)

                prompt_row = next(
                    index
                    for index, line in enumerate(observed["lines"])
                    if "Password:" in line
                )
                prompt_line = observed["lines"][prompt_row]
                expected_column = cell_len(
                    prompt_line[: prompt_line.index("Password:") + len("Password:")]
                )
                moves = re.findall(r"\x1b\[(\d+);(\d+)H", observed["controls"])
                self.assertEqual(password, "secret")
                self.assertEqual(
                    tuple(map(int, moves[-1])),
                    (prompt_row + 1, expected_column + 1),
                )
                self.assertNotIn("Password:", self.rendered_text(dashboard))

    def test_live_dashboard_cleans_up_password_modal_after_reader_error(self):
        jobs = load_jobs_config(config_path()).jobs[:1]
        console = terminal_console(width=80, height=18)
        dashboard = LiveDashboard(jobs, console=console)

        with self.assertRaisesRegex(RuntimeError, "reader failed"):
            dashboard.read_password(
                jobs,
                reader=lambda: (_ for _ in ()).throw(RuntimeError("reader failed")),
            )

        self.assertNotIn("Password:", self.rendered_text(dashboard))
        self.assertIn("\x1b[?25h", console.file.getvalue())
        self.assertTrue(console.file.getvalue().endswith("\x1b[?25l"))

    def test_password_modal_recenters_after_terminal_resize(self):
        jobs = load_jobs_config(config_path()).jobs[:1]
        console = MutableSizeConsole(width=120, height=24)
        dashboard = LiveDashboard(jobs, console=console)

        def observe_prompt_move():
            expected = None

            def reader():
                nonlocal expected
                size = console.size
                lines = console.render_lines(
                    dashboard.render(),
                    console.options.update(width=size.width, height=size.height),
                    pad=False,
                )
                for row, line in enumerate(lines):
                    text = "".join(segment.text for segment in line)
                    if "Password:" in text:
                        column = cell_len(
                            text[: text.index("Password:") + len("Password:")]
                        )
                        expected = (row + 1, column + 1)
                        break
                return "secret"

            dashboard.read_password(jobs, reader=reader)
            moves = re.findall(r"\x1b\[(\d+);(\d+)H", console.file.getvalue())
            actual = tuple(map(int, moves[-1]))
            console.file.seek(0)
            console.file.truncate()
            return expected, actual

        large_expected, large_actual = observe_prompt_move()
        console.set_size(width=80, height=18)
        small_expected, small_actual = observe_prompt_move()

        self.assertEqual(large_actual, large_expected)
        self.assertEqual(small_actual, small_expected)
        self.assertNotEqual(small_actual, large_actual)

    def test_password_cursor_ignores_password_text_in_job_name(self):
        jobs = [
            Job(
                name="Password: decoy",
                label="Decoy",
                phase="core",
                command=("decoy",),
                required_commands=(),
                optional=True,
                log_name="decoy.log",
            ),
            *load_jobs_config(config_path()).jobs[:12],
        ]
        console = terminal_console(width=120, height=18)
        dashboard = LiveDashboard(jobs, console=console)
        expected = None

        def reader():
            nonlocal expected
            size = console.size
            lines = console.render_lines(
                dashboard.render(),
                console.options.update(width=size.width, height=size.height),
                pad=False,
            )
            occurrences = []
            for row, line in enumerate(lines):
                text = "".join(segment.text for segment in line)
                if "Password:" in text:
                    column = cell_len(
                        text[: text.index("Password:") + len("Password:")]
                    )
                    occurrences.append((row + 1, column + 1))
            self.assertEqual(len(occurrences), 2)
            expected = occurrences[-1]
            return "secret"

        dashboard.read_password(jobs[1:2], reader=reader)
        moves = re.findall(r"\x1b\[(\d+);(\d+)H", console.file.getvalue())

        self.assertEqual(tuple(map(int, moves[-1])), expected)

    def test_sudo_auth_overlay_does_not_reflow_dashboard(self):
        jobs = load_jobs_config(config_path()).jobs
        console = terminal_console(width=160, height=25)
        dashboard = LiveDashboard(jobs, console=console)
        options = console.options.update(width=160, height=25)
        base_lines = [
            "".join(segment.text for segment in line)
            for line in console.render_lines(dashboard.render(), options, pad=False)
        ]
        overlay_lines = []

        def reader():
            overlay_lines.extend(
                "".join(segment.text for segment in line)
                for line in console.render_lines(dashboard.render(), options, pad=False)
            )
            return "secret"

        dashboard.read_password(
            [job for job in jobs if job.sudo_preflight], reader=reader
        )

        self.assertEqual(len(overlay_lines), 25)
        self.assertEqual(
            find_line_index(base_lines, "brew-link-node-tools"),
            find_line_index(overlay_lines, "brew-link-node-tools"),
        )
        prompt_line = overlay_lines[
            find_line_index(overlay_lines, "sudo authentication required")
        ]
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

        live.assert_called_once_with(
            dashboard,
            console=console,
            screen=True,
            auto_refresh=False,
            vertical_overflow="crop",
        )
        live.return_value.update.assert_called_once_with(dashboard, refresh=True)

    def test_live_dashboard_restores_terminal_when_entry_is_interrupted(self):
        jobs = load_jobs_config(config_path()).jobs[:1]

        class InterruptedConsole(Console):
            def set_alt_screen(self, enable: bool = True):
                changed = super().set_alt_screen(enable)
                if enable:
                    raise KeyboardInterrupt
                return changed

        console = InterruptedConsole(
            file=io.StringIO(),
            force_terminal=True,
            width=80,
            height=18,
            _environ={"TERM": "xterm-256color"},
        )
        dashboard = LiveDashboard(jobs, console=console)

        with self.assertRaises(KeyboardInterrupt):
            dashboard.__enter__()

        output = console.file.getvalue()
        self.assertEqual(output.count("\x1b[?1049h"), 1)
        self.assertEqual(output.count("\x1b[?1049l"), 1)
        self.assertGreater(output.rfind("\x1b[?25h"), output.rfind("\x1b[?25l"))

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


class TerminalHarnessTest(unittest.TestCase):
    def test_missing_password_prompt_does_not_match_missing_cursor_move(self):
        result = analyze(
            b"",
            scenario="password",
            width=80,
            height=6,
            echo_disabled=False,
        )

        self.assertFalse(result["password_cursor_matches_prompt"])


if __name__ == "__main__":
    unittest.main()
