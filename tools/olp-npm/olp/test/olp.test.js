// The small JS in the main package @feedpia/olp that picks the binary.
// Value: protects=runs only the binary of the per-OS package matching this OS and CPU (gui → olp-gui, else → olp-hook), and when missing stops with two different messages; fails_when=it runs another OS's binary, uses one message for «no build for this OS» and «not installed», or changes the arguments; why_new=one-line npx install; seam=plan() in bin/olp.js; no_claim=real npm install and publish are covered by the release workflow and local npm pack checks.
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { plan, PLATFORMS } = require('../bin/olp.js');

const found = (dir) => (name) => (name.endsWith('/package.json') ? path.join(dir, name.split('/')[1], 'package.json') : null);
const missing = () => null;

test('platform table names exactly the five packages in optionalDependencies', () => {
  const pkg = require('../package.json');
  const deps = Object.keys(pkg.optionalDependencies).sort();
  assert.deepEqual(deps, Object.values(PLATFORMS).sort());
  for (const v of Object.values(pkg.optionalDependencies)) assert.match(v, /^=\d+\.\d+\.\d+$/, '정확한 판 고정');
  assert.deepEqual(deps, ['@feedpia/olp-darwin-arm64', '@feedpia/olp-darwin-x64', '@feedpia/olp-linux-arm64', '@feedpia/olp-linux-x64', '@feedpia/olp-win32-x64']);
});

test('install goes to olp-hook.exe of the win32-x64 package with args unchanged', () => {
  const p = plan('win32', 'x64', ['install', '--dry-run'], found('/m'));
  assert.equal(p.error, undefined);
  assert.equal(p.file, path.join('/m', 'olp-win32-x64', 'bin', 'olp-hook.exe'));
  assert.deepEqual(p.args, ['install', '--dry-run']);
});

// olp-gui is not in the 0.1.0 packages → gui stops with one line of reason, no binary lookup
test('gui says the picker is not in this release', () => {
  const p = plan('darwin', 'arm64', ['gui', 'serve', '--lang', 'ko'], found('/m'));
  assert.equal(p.file, undefined);
  assert.match(p.error, /not in this release/);
});

test('package texts do not mention the picker', () => {
  const fs = require('node:fs');
  const root = path.join(__dirname, '..', '..');
  const texts = [path.join(root, 'olp', 'README.md'), ...['olp', 'olp-darwin-arm64', 'olp-darwin-x64', 'olp-linux-arm64', 'olp-linux-x64', 'olp-win32-x64'].map((d) => path.join(root, d, 'package.json'))];
  for (const f of texts) {
    const s = fs.readFileSync(f, 'utf8');
    assert.ok(!s.includes('olp-gui') && !s.includes('gui serve'), f);
  }
});

test('linux arm64 goes to olp-hook of the linux-arm64 package', () => {
  const p = plan('linux', 'arm64', ['install'], found('/m'));
  assert.equal(p.error, undefined);
  assert.equal(p.file, path.join('/m', 'olp-linux-arm64', 'bin', 'olp-hook'));
});

test('no arguments goes to olp-hook help, never to hook mode on stdin', () => {
  const p = plan('linux', 'x64', [], found('/m'));
  assert.equal(p.file, path.join('/m', 'olp-linux-x64', 'bin', 'olp-hook'));
  assert.deepEqual(p.args, ['--help']);
});

test('unknown os or cpu says there is no build yet', () => {
  for (const [os, cpu] of [['linux', 'ia32'], ['win32', 'arm64'], ['freebsd', 'x64']]) {
    const p = plan(os, cpu, ['install'], found('/m'));
    assert.equal(p.file, undefined);
    assert.match(p.error, /no build/);
    assert.ok(p.error.includes(`${os}-${cpu}`), p.error);
  }
});

test('known os whose package is not installed says to install again without omitting optional', () => {
  const p = plan('win32', 'x64', ['install'], missing);
  assert.equal(p.file, undefined);
  assert.match(p.error, /@feedpia\/olp-win32-x64/);
  assert.match(p.error, /--omit=optional/);
  assert.doesNotMatch(p.error, /no build/);
});

test('every os package.json matches its name, os and cpu and ships only bin/', () => {
  const root = path.join(__dirname, '..', '..');
  const main = require('../package.json');
  for (const [key, name] of Object.entries(PLATFORMS)) {
    const [os, cpu] = key.split('-');
    const pkg = require(path.join(root, name.split('/')[1], 'package.json'));
    assert.equal(pkg.name, name);
    assert.equal(pkg.version, main.version, '본 패키지와 같은 판');
    assert.deepEqual(pkg.os, [os]);
    assert.deepEqual(pkg.cpu, [cpu]);
    assert.deepEqual(pkg.files, ['bin/']);
    assert.equal(pkg.license, 'MIT');
    assert.equal(main.optionalDependencies[name], `=${main.version}`);
  }
});

// scoped packages default to restricted on npm — all five package.json files set public access (prevents 402 / private on first publish).
test('all five packages are published as public', () => {
  const root = path.join(__dirname, '..', '..');
  for (const name of ['@feedpia/olp', ...Object.values(PLATFORMS)]) {
    const pkg = require(path.join(root, name.split('/')[1], 'package.json'));
    assert.deepEqual(pkg.publishConfig, { access: 'public' }, name);
  }
});
