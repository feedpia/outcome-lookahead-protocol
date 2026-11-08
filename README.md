# olp-hook — see what a write would remove, before it runs

**0.1.1**

OLP (Outcome Lookahead Protocol) is an open specification for looking ahead at the outcome of an agent's action.

0.1.1: A Claude Code hook. Before a delete, its card lists which files and how many; before an overwrite, how many.

In Claude Code's permission prompt, before `rm -rf build` runs:

```text
[OLP] Before this runs: delete files — 3 files (preview: counted on this device, read-only) | Files: build/app.log, build/cache.tmp, build/notes.txt | before engine check · no receipt
```

```
npx @feedpia/olp install
```

### Guide video (2 min)

English subtitles:

https://github.com/user-attachments/assets/90489855-3ca4-4792-8889-cf647e7c7fd5

Korean subtitles:

https://github.com/user-attachments/assets/3d8dd4bc-a772-435c-88a5-6efbc3443e76

Recorded on 0.1.0. The card behaves the same in 0.1.1.

**Before an irreversible yes, see what it will change.**

Before an AI agent does something it can't undo, OLP counts what will be lost.

## Why

A command can change much more than it looks like it will. For example, joining two tables on the wrong key can turn 50 rows into 172, and nobody notices until later. The cheapest time to catch that is before the command runs, when you can still see how big the change is. Today the hook counts files and git history on this device; for database rows it shows only a number you give it.

OLP does not replace your agent or its permission prompt. It adds one step in front of them.

## Install and uninstall

In the project folder where you use Claude Code (Node.js 18 or later):

```
npx @feedpia/olp install
```

- It copies `olp-hook` to `olp` in your home folder (Windows `C:\Users\<you>\olp`), then adds one `olp-hook` entry each to `PreToolUse` and `PostToolUse` in `.claude/settings.json`. The hook then runs that copy directly, not through Node.
- Other entries and the order of keys stay as they are. Running it twice adds nothing. If the file is not valid JSON, nothing is changed.
- Before it writes, the old file is copied to `olp/backup` in your home folder (outside the project).
- At the end it shows one summary: the entries it added, what OLP does, how many of your 'always allow' permission rules (user, project and local settings) cover commands that can delete or overwrite, and how to undo it. It only reads those rules; it does not change them.
- `--dry-run` shows what would change without changing anything. `--user` uses your user settings (`~/.claude/settings.json`) instead. `--replace` points an existing `olp-hook` entry at the new copy.
- Run in your home folder without `--user`, install changes nothing and stops: the settings file there applies to every project. Run it inside a project folder, or add `--user` if you want that.
- `{"decision": true}` in `.olp/hook-config.json` (experimental, off by default): when a delete covers 2 or more files and some of them are not in git, the hook stops it once and gives the agent three options to ask you about (cancel first, then keep some, then delete all). Your choice is the agent's next instruction; the next delete shows the usual card and permission prompt.
- Update: re-run the install command (`npx @feedpia/olp install`). Version 0.1.1 has no self-update.
- Undo: `npx @feedpia/olp uninstall` removes only the `olp-hook` lines. The programs in `olp` are left; delete that folder if you no longer need them.
- The copy is checked byte for byte against the one in the package. That only shows the copy is intact; it does not show the package is ours. Check the release's `SHA256SUMS` for that.
- Downloaded from Releases instead? Unpack `olp-hook-<os>-<arch>.zip`/`.tar.gz` somewhere outside your project (the program inside is `olp-hook`, `olp-hook.exe` on Windows — do not rename it). On macOS, a browser download is blocked as unsigned until you run `xattr -d com.apple.quarantine <path>/olp-hook`. Then, inside your project, run that file with `install`: `~/Downloads/olp-hook install` (Windows: `~\Downloads\olp-hook.exe install`). It copies itself to `~/olp/` and writes the full path into `.claude/settings.json`. If the program was renamed, install changes nothing and stops with a one-line reason.

Builds: Windows x64, macOS (Apple silicon, Intel), Linux x64, Linux arm64. On other systems it stops and says there is no build yet.

## How it works

**Before engine check · no receipt.** The hook runs only on your device. The hook itself makes no network calls. Optional daily usage numbers (counts only) are described below; they are off by default and sent only if you say yes.

Setup is one line in your Claude Code settings (`.claude/settings.json`, `hooks` item). The same binary runs before and after each tool call:

```json
"hooks": { "PreToolUse": [ { "matcher": "Bash|PowerShell|mcp__.*", "hooks": [ { "type": "command", "command": "olp-hook", "timeout": 10, "onFailure": "block" } ] } ], "PostToolUse": [ { "matcher": "Bash|PowerShell|mcp__.*", "hooks": [ { "type": "command", "command": "olp-hook", "timeout": 10 } ] } ] }
```

This edition watches shell commands (`Bash`, `PowerShell`) and MCP tools only. File edits (Edit/Write) are not hooked here; Claude Code tracks them with its own checkpoints, and this hook makes no claim about them.

- **In the first week it may ask often.** Any command or tool it does not know yet is treated as a write. You get a card in Claude Code's own permission prompt, with a one-line command to classify it. After you answer, it remembers that answer for this project.
- **Choices you did not make are recorded on this device too.** Cards, calls it could not count, and cards after which the call did not run are written to `.olp/hook-log.jsonl` in the project folder.
- **Before engine check.** Writes and money moves always go to a person. Nothing here says a write is fine.

---

## What it does

Before each tool call, the hook sorts it into one of three effects: `read_only`, `writes_other`, `moves_money`.

| Call | Result |
|---|---|
| Built-in read tools (`Read`, `Glob`, `Grep`, `LS`, `NotebookRead`, `TodoWrite`) | passes |
| Shell commands on the closed read list (`ls`, `cat`, `head`, `tail`, `grep`, `rg`, `find` without write options, `git status/log/diff/show/ls-files/rev-parse/blame`, `echo`, `pwd`, `wc`, ...) | passes |
| The six risky forms in shell commands: delete files, overwrite files, git force-push (also deleting a remote branch), `git reset --hard`, delete database rows, change database rows | card in the permission prompt. A family answer cannot make these pass on their own. |
| Anything else | treated as a write, card in the permission prompt, with a classify line |

Where the card goes depends on the Claude Code permission mode:

- `default`, `plan`, `acceptEdits`, `auto` (the default in the desktop app): Claude Code shows its own permission prompt with the card. You choose.
- `bypassPermissions`, `dontAsk`, or any other or missing mode: these may skip the prompt, so the hook blocks the call instead and shows the card. In these modes OLP only stops; to go ahead, switch to a mode that asks (Shift+Tab) and answer the permission prompt.
- Hook errors, time-outs and crashes always block.

In the permission prompt the card is one line, and it always starts with `[OLP]` so you can tell it comes from OLP:

```
[OLP] Before this runs: delete files — 3 files (preview: counted on this device, read-only) | Files: build/a.txt, build/b.txt, build/c.log | before engine check · no receipt
```

The full card is the message shown next to the prompt. It looks like this:

```
OLP hook: check before it runs — before engine check · no receipt
What it does: delete files — 3 files (preview: counted on this device, read-only)
Files: build/a.txt, build/b.txt, build/c.log
Undo: not confirmed — no recovery location declared
Why: WRITE_IRREVERSIBLE, WRITE_RECOVERY_UNKNOWN
```

The file names show at most three, then "and N more". The count is a preview: it is read before the command runs and is not a promise. A command that only reads gets no card.

If you pick "Always allow" in the permission prompt, this card may not appear for the same command next time (being checked).

## Before a payment tool

Payment tools usually come through MCP (`mcp__...`), so the matcher above already sees them. Classify each payment tool once as `moves_money`, in each project folder that uses it. From then on, every call gets a card before it runs. Quiet mode does not pass it.

Use the tool names that your payment MCP server lists. The names below are placeholders.

**If your payment server is Stripe** (supported countries: see Stripe's official list):

```
olp-hook classify --tool mcp__<stripe-server>__<payment-tool> moves_money
```

**If your payment server is Kakao Pay** (a Korean payment service):

```
olp-hook classify --tool mcp__<kakaopay-server>__<payment-tool> moves_money
```

**If your payment server is Toss Payments** (a Korean payment service):

```
olp-hook classify --tool mcp__<tosspayments-server>__<payment-tool> moves_money
```

What the card looks like (real output, synthetic test folder, tool `mcp__payments__create_payment`):

```
OLP hook: check before it runs — before engine check · no receipt
What it does: moves money (classified by you) — count not checked
Amount: check it in the tool input before you answer (the hook does not read amounts)
Undo: not confirmed — no recovery location declared
Why: WRITE_IRREVERSIBLE, WRITE_RECOVERY_UNKNOWN
```

- For MCP tools the hook uses the tool name only, not the arguments. It does not store or pass on payment keys, card numbers, or amounts, and it never calls the payment server.
- The card does not say a payment is fine. You decide in Claude Code's permission prompt.
- The engine check (`check_action`) is not part of this first public release. It needs the OLP server, which is not open yet.
- Product names belong to their owners; no partnership is implied.

## Classify once (in your own terminal)

```
olp-hook classify --bash "cargo test" read_only
olp-hook classify --tool mcp__docs__search read_only
olp-hook classify --bash "make deploy" writes_other --reversible --recovery-declared
olp-hook classify --bash "psql" writes_other --db-count 42
```

- An answer covers the first word plus its subcommand (`cargo test`, `npm run`), not the rest of the line.
- `--reversible` only counts if you also declare a recovery location (`--recovery-declared`). Without one, the card always says "Undo: not confirmed".
- `--db-count` is a number you give. The card shows it as "user-given count · not checked by the guard". The hook never connects to a database.
- Forms that delete or overwrite (`rm`, `cp`, `mv`, ...) and `olp-hook` itself cannot be classified.

## Counting (device only)

Counting is on by default with the bundled limits (`data/hook-config.default.json`: counter 3000 ms, hook 5000 ms). To change them, set them in `.olp/hook-config.json`; a field you leave out keeps the bundled value. If counting runs past its limit, the card says "count not checked".

```json
{ "counter_timeout_ms": 3000, "hook_budget_ms": 5000 }
```

`counter_timeout_ms` must be smaller than `hook_budget_ms`. The Claude Code `timeout` above (in seconds) must be longer than `hook_budget_ms`. If the hook runs past its budget, it blocks.

What the counter may do (and nothing else): list paths without following links, `git status --porcelain`, `git rev-list --count <a>..<b>`. It never runs the original command.

Changing folder: `cd <folder> && …` (Bash) is counted in that folder only when the folder is written out as one plain word, already exists, and stays on the same drive. After `;`, `||`, `|`, `&` or a new line, or with a variable, `~`, `-`, `pushd` or PowerShell `Set-Location`, the card says "count not checked". One exception in PowerShell: when `cd`/`Set-Location` points at the project folder itself (its full path or `.`), the folder does not change, so the hook still counts.

## Quiet mode (per project, off by default)

Add `"quiet": true` to `.olp/hook-config.json`. Then a write you classified as `writes_other --reversible --recovery-declared` passes without a card. The pass is still recorded. Risky forms and money moves still get a card.

## Your numbers: `olp-hook stats`

Counts from this device only: first run time, decisions, stops (prompt or block), calls that ran after a card and calls that did not, quiet passes, classify answers. No paths, command text, or ids. Nothing is sent.

In a terminal, `stats` first draws this week (the last 7 UTC days) for about a second: cards in the prompt, ran after a card, not run after a card, blocked, and the files on those cards that were counted on this device. "Not run" means the call did not report back as run. Use `olp-hook stats --plain` (or set `NO_COLOR`) for plain text only. If your console shows stray characters, use `--plain`.

## Watch the card next to the prompt: `olp-hook watch`

Run `olp-hook watch` in another terminal tab, or `olp-hook watch --html [--lang ko]` and open the `file:///…/.olp/watch.html` link it prints. It shows the latest card (what it does, how many, undo status) and whether the call ran, reading only `.olp/hook-log.jsonl` on this device: no server, no port, no script in the page, nothing sent. `--once` draws one time and exits (no auto-refresh in the page).

## Optional daily usage numbers (off by default)

At the end of `npx @feedpia/olp install`, in a terminal, you are asked once: "Send anonymous daily usage numbers to FeedPia? (counts only, no commands or file names) [y/N]". Pressing Enter means no. Keys pressed before the question appears are dropped, so only an answer typed after it counts. Only a person typing `y` at this prompt says yes. No command option or environment variable says yes, and no file inside the project counts as a yes. When the install runs without a terminal (a pipe, CI or an agent) it does not ask and sends nothing. `olp-hook setup` asks the same question for a project you set up by hand, and is also how you turn it back on later. Only people who say yes send anything.

If you say yes, your answer and a random id are stored outside the project, in `<home>/olp/usage/` (one file per project folder, named by a hash of the folder path), and one `SessionStart` entry, `olp-hook signal --session-start`, is added to `.claude/settings.json`. When a new Claude Code session starts, it sends at most once a UTC day (00:00 UTC) these fields and nothing else:

| Field | What it is |
|---|---|
| `v` | format version (2) |
| `install_id` | the random id made when you said yes |
| `hook_version` | the olp-hook version |
| `install_path` | `npx` or `manual` |
| `used_today` | whether the hook ran since the last send (yes/no) |
| `age_days` | days since the hook first ran here, 0–31 (31 means 31 or more) |
| `cards_shown` | cards shown in the permission prompt |
| `blocked` | calls the hook blocked |
| `decision_on` | whether the decision card (`decision` setting) is on |
| `decision_asked` | decision cards shown |
| `after_decision` | after each decision card, whether the next delete the hook saw within 10 minutes counted `all` the files, `fewer`, or `none` came. This is what the hook saw next, not the answer the person chose. |

The counts cover what the hook recorded since the last send, up to 10 minutes before this send (so a decision card's 10 minutes are over before it is counted). Each count stops at 10000. No commands, paths, file or folder names, amounts or answer text are sent. The server does not store your IP address or the raw `install_id`. Server totals are by the day they were received; one daily send can cover several days since the last send.

It is sent only to the `https://` address in the bundled settings file, with no redirects, your own curl config file ignored, and a 2-second limit at session start (5 seconds when you run `olp-hook signal` yourself, which then says what happened: sent, no answer in time — it may still have arrived — or not sent). A project's `.olp/hook-config.json` can only turn it off (`"signal_endpoint": ""`); it cannot change the address. A failed send is not retried that day, prints nothing and never changes what the hook decides. The hook itself (PreToolUse/PostToolUse) never sends it.

Turn it off at any time with `npx @feedpia/olp telemetry off` (removes the `SessionStart` entry and your id). `OLP_HOOK_SIGNAL=off` also stops it and always wins.

## What is stored on this device

- `.olp/hook-map.json`: your answers. For each `tool:<name>` or `bash:<first word + subcommand>` it keeps the effect, whether it can be undone, whether a recovery location was declared, the time, and any count you gave. No arguments, paths, or amounts.
- `.olp/hook-log.jsonl`: what each card showed (kind, count, undo status, verdict, time, whether a person chose anything, and the Claude Code tool call id to pair a card with its run). No paths, command text, or database names.
- `.olp/first-run`, `.olp/used-day`: a time and a day number.
- Outside the project, `<home>/olp/usage/<hash>.json`: your answer to the usage-numbers question (and, if yes, the random id, `npx` or `manual`, and when it last sent).

On Unix the folder is `0700` and the files are `0600`. On Windows `.olp/` takes the folder permissions it is created in; this tool does not change them.

`olp-hook setup` also adds `.olp/` to this repository's `.git/info/exclude` (once), so these files are not committed by accident.

## Turn it off

Remove the `olp-hook` lines from your Claude Code settings (`olp-hook uninstall` does this, including the usage-numbers `SessionStart` entry). To stop only the usage numbers: `npx @feedpia/olp telemetry off`.

## Troubleshooting

- **Every shell command is blocked with a hook error:** Claude Code could not run `olp-hook` (missing, broken, or too slow), and `"onFailure": "block"` makes it block the call on purpose. Check that the `command` path in `.claude/settings.json` points to an `olp-hook` file that exists (by default in the `olp` folder in your home folder; `olp-hook.exe` on Windows).
- To turn it off now, remove the `hooks` entry this guide added from the project's `.claude/settings.json`. It stops in that project right away.

## Known limitations (0.1.1)

- OLP still asks before commands it doesn't recognize yet, even read-only ones. We add more with every release.
- On slow machines, PowerShell commands may trigger more prompts. Delete and overwrite cards still appear as before.
- Operator mode, which would handle recoverable actions without asking, isn't here yet. We're building it.

## Limits

- When the folder to delete is the working folder itself or one of its parents (for example `rm -rf /path/to/project` or `rm -rf .`), Claude Code shows its own "Dangerous rm" prompt and leaves OLP's card out of it, so one Yes would delete it without the file list. For that shape OLP stops the call instead of asking: nothing is deleted, the card goes to the agent, and it is told to rerun with a relative path or leave it to you in a terminal. PowerShell `Remove-Item` is not affected. Forms OLP cannot resolve to a folder, such as variables (`"$DIR"/*`), `bash -c "rm …"` or `/*` globs, still ask as before, so Claude Code's own prompt can still appear first for them.
- `"onFailure": "block"` on the `PreToolUse` entry makes Claude Code block the call if `olp-hook` is missing, fails, or runs past `timeout` (Claude Code v2.1.295 or later). Without it, or on an older Claude Code, the tool may run anyway. Keep the `timeout` longer than `hook_budget_ms`: the hook stops itself at its budget (5 seconds by default), so the 10-second `timeout` leaves room before Claude Code steps in.
- A family you mark `read_only` (for example `python`) passes as a whole. Only mark families that truly only read. Do not mark `git` itself `read_only`.
- This edition does not hook Edit/Write, so the agent can change `.olp/hook-map.json` or `.claude/settings.json` with its file-edit tools. The six risky forms are still caught, but your other answers could be changed. The next edition keeps the answers outside the project (in your home folder).
- For the same reason, an agent could use its file-edit tools to write the usage-numbers answer in `<home>/olp/usage/` and turn sending on. Only the numbers in the table above would be sent.
- "Not run" after a card means the call did not report back as run. It cannot tell a "no" in the prompt from a failed run.
- Counts are a read-only preview. They are not a promise of exactly what would disappear.
- In PowerShell (what Claude Code uses on Windows), the hook counts `Remove-Item` and its short names (`del`, `rd`, `ri`, `rm`, `rmdir`, `erase`) when the paths are written out. Variables, wildcards, piped input, `-Filter`/`-Include`/`-Exclude`/`-WhatIf`, and non-file paths such as `HKCU:` or `Env:` show "count not checked". Read-only commands such as `Get-ChildItem`, `Test-Path` and `Measure-Object` pass without a card.
- In PowerShell, a line that is only a quoted string (such as `"--- build ---"`) just prints text, so it passes without a card. A string with `$(…)` inside, or one run with `&` or `.`, is not treated this way. `2>$null` and `>$null` throw output away and count as no file write.
- On Windows, when a PowerShell command has `{ }` blocks (such as `Where-Object` or `ForEach-Object`), the hook reads it with PowerShell's own syntax parser. The parser only reads the text and never runs it. Commands, property reads and string methods inside the blocks are checked; a file-changing method such as `.Delete()` shows a card, and anything it does not recognize is treated as a write, with a "Not recognized:" line. Where that parser is not available (macOS, Linux, or if it takes over 1.5 seconds), such commands are treated as a write. To turn the parser off, set the environment variable `OLP_HOOK_PS_PARSER=off` for Claude Code; the hook then treats these commands as a write.

License: MIT. 한국어: [README.ko.md](README.ko.md)
