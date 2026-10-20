"""Update moving Rust channels, then retire fixed-version toolchains."""

import subprocess


def main() -> int:
    # Never remove the old compilers unless both replacement channels work.
    result = subprocess.run(["rustup", "update", "stable", "nightly"], check=False)
    if result.returncode:
        return result.returncode

    for channel in ("stable", "nightly"):
        result = subprocess.run(
            ["rustup", "run", channel, "rustc", "--version"], check=False
        )
        if result.returncode:
            return result.returncode

    installed = subprocess.run(
        ["rustup", "toolchain", "list"], capture_output=True, text=True, check=False
    )
    if installed.returncode:
        print(installed.stderr, end="")
        return installed.returncode

    default = subprocess.run(
        ["rustup", "default"], capture_output=True, text=True, check=False
    )
    if default.returncode:
        print(default.stderr, end="")
        return default.returncode

    # Only replace a fixed-version default that is about to be removed.
    if default.stdout.split()[0].split("-", 1)[0].replace(".", "").isdigit():
        result = subprocess.run(["rustup", "default", "stable"], check=False)
        if result.returncode:
            return result.returncode

    for line in installed.stdout.splitlines():
        name = line.split()[0]
        # Preserve custom/linked toolchains. Only rustup release names qualify.
        version = name.split("-", 1)[0]
        if not version.replace(".", "").isdigit():
            continue

        result = subprocess.run(["rustup", "toolchain", "uninstall", name], check=False)
        if result.returncode:
            return result.returncode

    return 0
