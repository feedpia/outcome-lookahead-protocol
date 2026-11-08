#!/usr/bin/env node
// @feedpia/olp — a small JS that picks the binary inside the per-OS package (@feedpia/olp-<os>-<cpu>).
// What it does: passes the arguments to olp-hook as is and exits with its exit code (`gui` = not in this version). No dependencies, no network.
// The hook itself does not go through Node: `install` copies the binary to a fixed place and writes that path into settings.json.
'use strict';
const path = require('node:path');

/** `<process.platform>-<process.arch>` → package name. A combination not listed here = no build yet. */
const PLATFORMS = {
  'win32-x64': '@feedpia/olp-win32-x64',
  'darwin-arm64': '@feedpia/olp-darwin-arm64',
  'darwin-x64': '@feedpia/olp-darwin-x64',
  'linux-x64': '@feedpia/olp-linux-x64',
  'linux-arm64': '@feedpia/olp-linux-arm64',
};

/**
 * Decides what to run (does not run it). resolve(spec) = path to that package's package.json, or null.
 * @returns {{file?: string, args?: string[], error?: string}}
 */
function plan(platform, arch, args, resolve) {
  const key = `${platform}-${arch}`;
  const name = PLATFORMS[key];
  if (!name) {
    return { error: `olp: there is no build for ${key} yet (available: ${Object.keys(PLATFORMS).join(', ')}).` };
  }
  const manifest = resolve(`${name}/package.json`);
  if (!manifest) {
    return { error: `olp: the package for your system (${name}) was not installed. Install again without --omit=optional or --no-optional.` };
  }
  // olp-gui is not in the 0.1.0 packages → do not look for a binary, give one line of reason
  if (args[0] === 'gui') {
    return { error: 'olp: the file picker is not in this release. Read the card, then tell the agent what to change.' };
  }
  const exe = 'olp-hook' + (platform === 'win32' ? '.exe' : '');
  // called with no arguments, olp-hook would wait on stdin in hook mode, so show help instead
  const rest = args.length === 0 ? ['--help'] : args;
  return { file: path.join(path.dirname(manifest), 'bin', exe), args: rest };
}

function main() {
  const resolve = (spec) => {
    try {
      return require.resolve(spec);
    } catch {
      return null;
    }
  };
  const p = plan(process.platform, process.arch, process.argv.slice(2), resolve);
  if (p.error) {
    process.stderr.write(`${p.error}\n`);
    process.exit(1);
  }
  const r = require('node:child_process').spawnSync(p.file, p.args, { stdio: 'inherit' });
  if (r.error) {
    process.stderr.write(`olp: could not run ${p.file}\n`);
    process.exit(1);
  }
  process.exit(r.status === null ? 1 : r.status);
}

if (require.main === module) main();

module.exports = { plan, PLATFORMS };
