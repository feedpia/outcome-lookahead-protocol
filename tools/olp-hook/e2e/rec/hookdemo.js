// For the video: take the hook command registered in .claude/settings.json as is and run it with the same PreToolUse input
// Claude Code sends right before using a tool, then show the result as is (nothing made up). Usage: node hookdemo.js "<shell command>"
'use strict';
const fs = require('node:fs');
const { spawnSync } = require('node:child_process');

const tcmd = process.argv[2];
const s = JSON.parse(fs.readFileSync('.claude/settings.json', 'utf8'));
const hook = s.hooks.PreToolUse.flatMap((e) => e.hooks).find((h) => /olp-hook(\.exe)?"?$/.test(h.command));
const input = JSON.stringify({
  session_id: 'demo', transcript_path: 'demo', cwd: process.cwd(), hook_event_name: 'PreToolUse',
  tool_name: 'Bash', tool_input: { command: tcmd }, permission_mode: 'default',
});
console.log(`> Claude Code wants to run: ${tcmd}`);
const r = spawnSync(hook.command, { input, shell: true, encoding: 'utf8' });
const out = (r.stdout || '').trim();
if (!out) {
  console.log('OLP: pass (no card, the command runs as usual)');
} else {
  const v = JSON.parse(out);
  console.log(`OLP: ${v.hookSpecificOutput.permissionDecision} (card shown in the permission prompt)`);
  console.log(v.systemMessage);
}
process.exit(r.status === null ? 1 : r.status);
