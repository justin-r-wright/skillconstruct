# Skillet

One window for every agent skill on this machine: what is installed, in which harness
(`~/.agents`, `~/.claude`, `~/.codex`, `~/.cursor`, `~/.config/opencode`), what is broken,
what never gets used, plus add / link / update / remove and "ask an agent if I need this".

    cargo run --release     # launch from a terminal so it inherits your PATH (npx, claude, codex)
    cargo test

- Removing a real folder moves it to `~/.Trash`; symlinks are just deleted.
- Updates and remote installs shell out to `npx skills` (the same tool that wrote `~/.agents/.skill-lock.json`).
- Usage counts come from Claude Code transcripts only (`~/.claude/projects`).
- "Ask an agent" runs `claude -p`, `codex exec`, or `opencode run` on your existing logins.
