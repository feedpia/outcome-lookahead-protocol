// Self-test for the e2e receipt pieces (DESIGN_OLP_013_TAG_GATE §6 E-meets · E-src, second review C3).
// Run: node --test tools/olp-hook/e2e/test_receipt_parts.js   (olp_e2e.sh runs it first, so a broken piece fails the e2e run)
"use strict";
const test = require("node:test");
const assert = require("node:assert");
const fs = require("fs");
const path = require("path");
const { runtimeText, rulesText, meets, EXPECT } = require("./receipt_parts.js");

const RUNTIME_LINE = 'const RUNTIME_TARGET: &str = "the target is only known at run time";';
const RULES_SRC = [
  '        1 => ("1 \'always allow\' rule".to_string(), "covers"),',
  '    format!("Permission rules: {n} {verb} some of these commands …")',
].join("\n");

const good = () => ({ install: 0, hooks: 2, card_delete: 1, card_overwrite: 1, uninstall: 0, hooks_after: 0,
  replay_exit: 0, replay_stopped: 1, replay_untouched: 1, card_delcard: 4, reason_runtime: 1, rules_line: 1, marker_cleanup: 1 });

test("E-meets: every key at its expected value meets", () => {
  assert.strictEqual(meets(good()), true);
});

test("E-meets: the expected table names all 13 receipt keys", () => {
  assert.deepStrictEqual(Object.keys(EXPECT).sort(), Object.keys(good()).sort());
});

test("E-meets: any one key off by one, zero, or missing does not meet", () => {
  for (const k of Object.keys(good())) {
    for (const bad of [good()[k] + 1, good()[k] === 0 ? 1 : 0, undefined, NaN]) {
      const c = good();
      if (bad === undefined) delete c[k]; else c[k] = bad;
      assert.strictEqual(meets(c), false, `${k} = ${bad}`);
    }
  }
});

test("E-src: the RUNTIME_TARGET line is read, LF or CRLF", () => {
  assert.strictEqual(runtimeText(RUNTIME_LINE + "\n"), "the target is only known at run time");
  assert.strictEqual(runtimeText("x\r\n" + RUNTIME_LINE + "\r\nfn y() {}\r\n"), "the target is only known at run time");
});

test("E-src: zero or two RUNTIME_TARGET lines give empty text (so reason_runtime stays 0)", () => {
  assert.strictEqual(runtimeText("fn main() {}\n"), "");
  assert.strictEqual(runtimeText(RUNTIME_LINE + "\n" + RUNTIME_LINE + "\n"), "");
  assert.strictEqual(runtimeText('const RUNTIME_TARGET: &str = "x"; // moved\n'), "");
});

test("E-src: the rules line is read only when both pieces are there once", () => {
  assert.strictEqual(rulesText(RULES_SRC), "Permission rules: 1 'always allow' rule covers");
  assert.strictEqual(rulesText(RULES_SRC.split("\n")[0]), "");
  assert.strictEqual(rulesText(RULES_SRC + "\n" + RULES_SRC), "");
});

test("E-meets: EXPECT is the thresholds file e2e + e2e_013, key for key and value for value", () => {
  const th = JSON.parse(fs.readFileSync(path.join(__dirname, "..", "qual", "tag_gate_thresholds.json"), "utf8"));
  assert.deepStrictEqual({ ...EXPECT }, { ...th.e2e, ...th.e2e_013 });
});
