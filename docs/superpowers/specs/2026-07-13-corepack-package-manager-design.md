# Corepack Package Manager Design

## Goal

Make Corepack the sole dispatcher for pnpm and Yarn while allowing `sup` to
upgrade the unpinned pnpm default to the latest stable release automatically.
Projects that pin a package manager version must continue using that version.

## Current Problem

Homebrew pnpm and npm-installed Corepack both create `/opt/homebrew/bin/pnpm`.
The `brew` alias upgrades and relinks Homebrew pnpm during the core phase, but
the parallel `npm update --global` job can replace that link with Corepack's
shim. The concurrent `pnpm update --global` job can therefore run through an
older Corepack default while npm is changing the shim.

## Design

Corepack will own the `pnpm`, `pnpx`, `yarn`, and `yarnpkg` shims. Homebrew will
continue to own Node but will no longer install or relink pnpm.

The existing npm and pnpm update jobs will become one ordered Node package
manager job:

1. Run `npm update --global` to update Corepack and other npm globals.
2. Run `corepack install --global pnpm@latest` to select the latest stable pnpm
   as Corepack's default for directories without a package-manager pin.
3. Run `pnpm update --global` to update packages in pnpm's global store.

The commands will run in a single shell job joined with `&&`. A failed command
will stop the sequence, preventing later commands from running through stale or
partially updated package-manager state.

The Homebrew link-repair jobs will retain their existing positions but manage
only Node. This keeps the existing defense against overwritten Node links while
removing pnpm's competing ownership.

## Migration

The local machine requires a one-time migration after the repository change:

1. Remove the Homebrew pnpm formula.
2. Remove the legacy global `@pnpm/exe` package.
3. Update and enable Corepack.
4. Select `pnpm@latest` as Corepack's unpinned default.

The migration changes local developer tooling only. It does not modify package
manager pins or lockfiles in application repositories.

## Verification

Automated tests will assert that:

- Homebrew link-repair jobs manage Node without pnpm.
- npm, Corepack, and pnpm global updates are represented by one ordered job.
- the old independent pnpm update job is absent.
- selector expansion and dry-run output include the replacement job.

Repository verification will use the commands required by `AGENTS.md`.

Live verification will confirm that:

- an unpinned directory runs the latest stable pnpm;
- `chief` runs its pinned pnpm 11.9.0;
- `venari` runs its pinned pnpm 11.8.0;
- `ts-proto` runs its pinned Yarn 4.4.0; and
- `sup --dry-run` shows the ordered Node package-manager update.

## Out of Scope

- Changing package-manager pins in application repositories.
- Automatically upgrading a project's pinned pnpm or Yarn version.
- Introducing another package manager dispatcher such as Volta or mise.
