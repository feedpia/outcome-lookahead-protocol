//! olp-hook — decision component (lib) for the Claude Code `PreToolUse` hook. The executable (main.rs) only reads stdin and sets the exit code.
//! A future MCP wrapper would also use this lib's `classify` as is, so both entry points share one classification table.
//! This crate makes no network calls (the `hook_makes_no_network_calls` test checks the source and dependency list).

pub mod card;
pub mod classify;
pub mod config;
pub mod counter;
/// Decision card experiment (setting `decision`, default off): blocks a deletion once and gives the agent 3 options.
pub mod decision;
pub mod hook;
/// `install`/`uninstall`: add or remove our two hook entries in settings.json and copy the executable to a fixed location. Never called from the hook or counting paths.
pub mod install;
/// "Pick what to keep" (setting `pick`, only while `olp-gui` is running): does not change the decision, only adds one card line and a pending temp file.
pub mod pick;
/// PowerShell syntax-parser bridge (no execution): used only when the Rust parser cannot reach a conclusion, Windows only; on failure falls back to the current list.
pub mod ps_ast;
/// Reads broad "always allow" rules from user, project and local settings (read-only). Called only for the end-of-install notes.
pub mod rules;
pub mod shell;
/// Opt-in one-bit signal (default off). Hook and counting paths never call this module; only a person via `olp-hook signal`.
pub mod signal;
pub mod stats;
pub mod store;
/// Companion view (`olp-hook watch`) — reads the record only. Hook and counting paths never call this module.
pub mod watch;

pub use hook::{classify_cli, guarded, run_hook, GuardFail, HookOutcome};
