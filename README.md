# Skill Construct

A small desktop app for managing agent skills (`SKILL.md` folders) across every coding
agent on your machine. One window shows what is installed, where, what is broken and
what you never use.

Built with Rust and [egui](https://github.com/emilk/egui). macOS is the tested platform;
Linux should work but is untested. Windows is not supported.

## What it does

- **See everything.** Scans `~/.agents/skills` (the shared folder `npx skills` installs into),
  `~/.claude/skills`, `~/.codex/skills`, `~/.cursor/skills` and `~/.config/opencode/skills`,
  plus read-only skills shipped by Claude Code plugins.
- **Spot problems.** Broken links, missing `SKILL.md`, bad or missing name and description,
  and copies that have drifted apart between harnesses.
- **Find dead weight.** Counts how often each skill was invoked in your Claude Code transcripts.
- **Clean up.** Remove one skill, or everything from one source, in two clicks. Real folders
  go to the Trash; links are just deleted.
- **Share across harnesses.** Link a skill into any harness that lacks it.
- **Add and update.** Add from a local folder, `owner/repo` or URL; update from source.
- **Ask an agent.** "Do I still need this?" goes to `claude`, `codex` or `opencode` using
  the logins you already have, with the skill and the rest of your library as context.

## Install

**macOS:** download the latest zip from
[Releases](https://github.com/justin-r-wright/skillconstruct/releases/latest), unzip it and drag
`Skill Construct.app` to Applications. The app is a universal build (Apple Silicon and Intel),
signed and notarized.

**From source** (macOS or Linux), with a recent Rust toolchain:

    cargo run --release

Updating and adding remote skills needs Node (`npx`).
Asking an agent needs that agent's CLI installed and logged in.

## How it works

- Remote installs and updates shell out to [`npx skills`](https://github.com/vercel-labs/skills).
  Removing a skill also drops it from `~/.agents/.skill-lock.json`, so an update will not bring it back.
- Usage counts come from Claude Code transcripts only (`~/.claude/projects`). Other agents
  are not counted, so "never used" means "never used in Claude Code".
- Agents are asked read-only: `claude -p --tools ""`, `codex exec -s read-only`, `opencode run`.
- Nothing leaves your machine except through those tools.

## Develop

    cargo test

`src/core.rs` holds scanning and file operations with no UI, tested against a throwaway
home directory. `src/main.rs` is the egui front end.

`scripts/release-macos.sh` builds the signed macOS app into `dist/`. It needs a Developer ID
certificate, and a `notarytool` keychain profile passed as `NOTARY_PROFILE` to notarize.

## License

MIT
