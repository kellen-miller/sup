import subprocess
import unittest
from unittest.mock import patch

from sup.rust import main


class RustUpdateTests(unittest.TestCase):
    def test_only_release_toolchains_are_removed_after_channel_validation(self):
        def run(command, **kwargs):
            output = ""
            if command == ["rustup", "toolchain", "list"]:
                output = (
                    "stable-aarch64-apple-darwin (active, default)\n"
                    "nightly-aarch64-apple-darwin\n"
                    "1.93.0-aarch64-apple-darwin\n"
                    "custom-compiler\n"
                )

            if command == ["rustup", "default"]:
                output = "nightly-aarch64-apple-darwin (default)\n"

            return subprocess.CompletedProcess(command, 0, stdout=output, stderr="")

        with patch("sup.rust.subprocess.run", side_effect=run) as mocked:
            self.assertEqual(main(), 0)

        commands = [call.args[0] for call in mocked.call_args_list]
        self.assertEqual(commands[0], ["rustup", "update", "stable", "nightly"])
        self.assertEqual(
            commands[-1],
            ["rustup", "toolchain", "uninstall", "1.93.0-aarch64-apple-darwin"],
        )
        self.assertEqual(len(commands), 6)
        self.assertNotIn(["rustup", "default", "stable"], commands)

    def test_fixed_version_default_moves_before_uninstall(self):
        commands = []

        def run(command, **kwargs):
            commands.append(command)
            output = ""
            if command == ["rustup", "toolchain", "list"]:
                output = "stable-host\nnightly-host\n1.93.0-host\n"
            elif command == ["rustup", "default"]:
                output = "1.93.0-host (default)\n"

            return subprocess.CompletedProcess(command, 0, stdout=output, stderr="")

        with patch("sup.rust.subprocess.run", side_effect=run):
            self.assertEqual(main(), 0)

        self.assertEqual(
            commands[-2:],
            [
                ["rustup", "default", "stable"],
                ["rustup", "toolchain", "uninstall", "1.93.0-host"],
            ],
        )

    def test_failed_update_or_validation_never_removes_toolchains(self):
        for failure_at in range(5):
            with self.subTest(failure_at=failure_at):
                results = [
                    subprocess.CompletedProcess([], 0) for _ in range(failure_at)
                ]
                results.append(subprocess.CompletedProcess([], 1, stderr="failed"))
                with (
                    patch("sup.rust.subprocess.run", side_effect=results) as mocked,
                    patch("builtins.print"),
                ):
                    self.assertEqual(main(), 1)

                self.assertFalse(
                    any("uninstall" in call.args[0] for call in mocked.call_args_list)
                )
