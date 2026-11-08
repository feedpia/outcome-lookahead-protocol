// For the video: show only the hook entries the installer wrote to .claude/settings.json (file read as is).
'use strict';
const s = JSON.parse(require('node:fs').readFileSync('.claude/settings.json', 'utf8'));
console.log(JSON.stringify({ hooks: { PreToolUse: s.hooks.PreToolUse } }, null, 2));
