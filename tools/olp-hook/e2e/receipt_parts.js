// Pieces of the e2e receipt that olp_e2e.sh uses (DESIGN_OLP_013_TAG_GATE §2-1 · §6), kept here so test_receipt_parts.js can test them.
//   node receipt_parts.js runtime <classify.rs>   prints the RUNTIME_TARGET text (one match, or nothing)
//   node receipt_parts.js rules <rules.rs>         prints "Permission rules: 1 'always allow' rule covers" (both pieces once, or nothing)
"use strict";
const fs = require("fs");

// Same values as tools/olp-hook/qual/tag_gate_thresholds.json e2e + e2e_013, so ok in the receipt = the gate recompute.
const EXPECT = Object.freeze({
  install: 0, hooks: 2, card_delete: 1, card_overwrite: 1, uninstall: 0, hooks_after: 0,
  replay_exit: 0, replay_stopped: 1, replay_untouched: 1, card_delcard: 4, reason_runtime: 1, rules_line: 1, marker_cleanup: 1,
});

function meets(counts) {
  return Object.keys(EXPECT).every((k) => counts[k] === EXPECT[k]);
}

function runtimeText(src) {
  const m = [...src.matchAll(/^const RUNTIME_TARGET: &str = "([^"]+)";\r?$/gm)];   // \r: Windows checkouts may turn line ends into CRLF
  return m.length === 1 ? m[0][1] : "";
}

function rulesText(src) {
  const n = [...src.matchAll(/\("(1 'always allow' rule)"\.to_string\(\), "(covers)"\)/g)];
  const f = [...src.matchAll(/format!\("(Permission rules:) \{n\} \{verb\} /g)];
  return n.length === 1 && f.length === 1 ? f[0][1] + " " + n[0][1] + " " + n[0][2] : "";
}

module.exports = { EXPECT, meets, runtimeText, rulesText };

if (require.main === module) {
  const [what, file] = process.argv.slice(2);
  let t = "";
  try { t = fs.readFileSync(file, "utf8"); } catch (e) { process.exit(0); }   // unreadable source = empty text = that key stays 0
  if (what === "runtime") process.stdout.write(runtimeText(t));
  else if (what === "rules") process.stdout.write(rulesText(t));
}
