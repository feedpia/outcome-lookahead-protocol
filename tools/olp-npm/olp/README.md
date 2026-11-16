# @feedpia/olp

Outcome Lookahead Protocol (OLP): before an AI agent's command runs, see what it would remove and how many.

```
npx @feedpia/olp install
```

Run it in the project folder where you use Claude Code (Node.js 18 or later). It copies `olp-hook` to `olp` in your home folder and adds one `olp-hook` entry each to `PreToolUse` and `PostToolUse` in `.claude/settings.json`. Other entries stay as they are; the old file is backed up to `olp/backup` in your home folder. `--dry-run` shows the change without making it.

- Undo: `npx @feedpia/olp uninstall` removes only the `olp-hook` lines.
- The hook runs on your device. The hook itself makes no network calls.
- Usage numbers are off by default. At the end of `npx @feedpia/olp install`, in a terminal, you are asked once `[y/N]`; Enter means no. Run without a terminal (a pipe, CI or an agent), it does not ask and sends nothing. Only if you type `y` does it send counts with a random per-project ID, at most once a day, never commands or file names. Turn off: `npx @feedpia/olp telemetry off`. The fields are listed in the repository README.

Builds: Windows x64, macOS (Apple silicon, Intel), Linux x64, Linux arm64. Full guide: the `olp-hook` README in the repository.

License: MIT.
