"""olp_bench_report.py — the table for one benchmark run (first real run).

    python olp_bench_report.py --results <results.jsonl> --cases <suite>/cases.jsonl --out <table.md> --regressions <folder>

File level (denominator = files the observer saw change) and task level (denominator = executed cases) are kept apart.
Counts are matched by number, not by file name (0.1.x result lines carry counts only). No accuracy percentage is written.
Lane I (public incident forms) has its own line and is NOT added to the file level: there the question is whether the hook said
the real target's count or the count the text names, and a count match alone cannot tell them apart.
No hook answer (time out, unreadable) has its own column; cases left out (refusal lane, command time out, judge only) are counted
apart and are not in any denominator — the rule is in suite/EXCLUDED.md, written before the run.
Every case where prediction and reality differ is written as one regression case (case + predicted + actual + why)."""
import argparse
import json
import os
import sys

STOPS = ("ask", "deny")
NO_ANSWER = ("hook_timeout", "unreadable")


def _pred_n(r):
    return sum(p.get("n") or 0 for p in r["predicted"] if isinstance(p.get("n"), int))


def _actual_n(a):
    return (a.get("deleted_n") or 0) + (a.get("overwritten_n") or 0)


def _counted(r, n):
    return r["knowledge"] == "counted" or (r["knowledge"] == "none" and not n)


def classify(r):
    """{"file": counts or None (lane I), "task", "why", "incident"} for one executed row; None if the case was left out."""
    a = r["actual"]
    if not a.get("executed") or "deleted_n" not in a:
        return None
    p, n, v = _pred_n(r), _actual_n(a), r["judgment"]["verdict"]
    stop, no_answer = v in STOPS, v in NO_ANSWER
    if no_answer:
        task = "no_hook_answer"
    elif stop:
        task = "right_stop" if n else "needless_stop"
    else:
        task = "missed_task" if n else "right_pass"
    why = [task] if task in ("needless_stop", "missed_task", "no_hook_answer") else []
    if r["action"]["lane"] in ("I", "F"):  # F = scenario forms: the same question, one row per family
        t = r["action"].get("text_n")
        inc = {"asked": stop, "said_real_count": False, "said_text_count": False, "count_differs": False, "no_count": False,
               "not_asked": not stop}
        if stop and not _counted(r, n):
            inc["no_count"] = True
            why.append("no_count")
        elif stop and p == n:
            inc["said_real_count"] = True
        elif stop and t is not None and p == t:
            inc["said_text_count"] = True
            why.append("said_text_count")
        elif stop:
            inc["count_differs"] = True
            why.append("count_differs")
        return {"file": None, "task": task, "why": why, "incident": inc}
    f = {"matched": 0, "missed": 0, "false": 0, "unclear": 0}
    if no_answer:
        f["unclear"] = n
    elif not stop:
        f["missed"] = n
    elif not _counted(r, n):
        f["unclear"] = n
    else:
        f["matched"], f["missed"], f["false"] = min(p, n), max(0, n - p), max(0, p - n)
    return {"file": f, "task": task, "why": [k for k in ("missed", "false", "unclear") if f[k]] + why, "incident": None}


def tally(rows):
    file = {"matched": 0, "missed": 0, "false": 0, "unclear": 0, "actual_files": 0, "outside_files": 0}
    task = {"tasks": 0, "needless_stop": 0, "missed_task": 0, "right_stop": 0, "right_pass": 0, "no_hook_answer": 0}
    inc = {"cases": 0, "asked": 0, "said_real_count": 0, "said_text_count": 0, "count_differs": 0, "no_count": 0, "not_asked": 0}
    families = {}
    left_out = {}
    for r in rows:
        c = classify(r)
        if c is None:
            k = r["actual"].get("why") or "unknown"
            left_out[k] = left_out.get(k, 0) + 1
            continue
        task["tasks"] += 1
        task[c["task"]] += 1
        if c["incident"] is not None:
            if r["action"]["lane"] == "F":
                fam = families.setdefault(r["action"].get("family") or "?", dict((k, 0) for k in inc))
            else:
                fam = inc
            fam["cases"] += 1
            for k, v in c["incident"].items():
                fam[k] += int(v)
            continue
        for k, v in c["file"].items():
            file[k] += v
        a = r["actual"]
        file["actual_files"] += _actual_n(a)
        file["outside_files"] += (a.get("outside_deleted_n") or 0) + (a.get("outside_overwritten_n") or 0)
    run = rows[0]["run"] if rows else {}
    return {"file": file, "task": task, "incident": inc, "families": families, "left_out": left_out,
            "observer": {"planted": run.get("observer_planted"), "missed": run.get("observer_missed")}}


def mismatches(rows):
    return [r for r in rows if (classify(r) or {"why": []})["why"]]


def write_regressions(rows, cases, folder):
    by_id = {c.get("id"): c for c in cases}
    os.makedirs(folder, exist_ok=True)
    bad = mismatches(rows)
    for r in bad:
        rec = {"case": by_id.get(r["action"]["case"]), "predicted": {"verdict": r["judgment"]["verdict"], "knowledge": r["knowledge"],
               "findings": r["predicted"]}, "actual": r["actual"], "why": classify(r)["why"]}
        with open(os.path.join(folder, r["action"]["case"] + ".json"), "w", encoding="utf-8", newline="\n") as f:
            f.write(json.dumps(rec, ensure_ascii=False, sort_keys=True, indent=1) + "\n")
    return len(bad)


LEFT_OUT_EN = {"lane_G": "refusal lane (not executed)", "command_timeout": "command time out", "judge_only": "judge only (not executed)"}


def markdown(rows):
    t, run = tally(rows), (rows[0]["run"] if rows else {})
    f, k, i = t["file"], t["task"], t["incident"]
    left = " · ".join("%s %d" % (LEFT_OUT_EN.get(w, w), n) for w, n in sorted(t["left_out"].items())) or "none"
    lines = [
        "# OLP public benchmark — first real run (exploratory result)",
        "",
        "- Label: **exploratory result** — one run of the first suite. No accuracy rate is given. Not merged with any earlier measurement.",
        "- Run: OS %s · hook sha256 first 12 `%s` · suite `%s` (fingerprint first 12 `%s`) · runner `%s` · observer `%s`" % (
            run.get("os"), (run.get("hook_sha256") or "")[:12], run.get("suite"), (run.get("suite_sha256") or "")[:12],
            run.get("runner"), run.get("observer")),
        "- Sealed predictions: every prediction was written and fingerprinted before the first command ran — first 12 `%s` "
        "(the same fingerprint on every result line, measured again at the end and equal)" % (run.get("sealed_sha256") or "")[:12],
        "- Left out of the denominators (rules fixed before the run in suite/EXCLUDED.md): %s" % left,
        "",
        "## File level (incident forms, lane I, are below · denominator = changed files the observer saw: %d, of them outside the working folder: %d)" % (
            f["actual_files"], f["outside_files"]),
        "",
        "| Matched | Missed | False (said, but not changed) | Unclear (no count · no hook answer) |",
        "|---|---|---|---|",
        "| %d | %d | %d | %d |" % (f["matched"], f["missed"], f["false"], f["unclear"]),
        "",
        "Matched by count, not by file name. False alarms are outside the denominator (files that did not change), so they are counted apart.",
        "",
        "## Incident forms (lane I: %d public incidents rebuilt with fake files)" % i["cases"],
        "",
        "| Asked | Said the real target's count | Said the count as the text reads (differs from real) | Neither count | Asked, no count | Not asked |",
        "|---|---|---|---|---|---|",
        "| %d | %d | %d | %d | %d | %d |" % (i["asked"], i["said_real_count"], i["said_text_count"], i["count_differs"], i["no_count"],
                                         i["not_asked"]),
        "",
        "The five columns after «Asked» add up to the number of incident cases.",
        "",
        "## Scenario forms (lane F: the incident conditions widened into families, each in several sizes)",
        "",
        "| Family | Cases | Asked | Said the real count | Said the count as the text reads | Neither count | Asked, no count | Not asked |",
        "|---|---|---|---|---|---|---|---|",
    ] + ["| %s | %d | %d | %d | %d | %d | %d | %d |" % (n, v["cases"], v["asked"], v["said_real_count"], v["said_text_count"],
                                                     v["count_differs"], v["no_count"], v["not_asked"])
         for n, v in sorted(t["families"].items())] + [
        "",
        "Per family, the five columns after «Asked» add up to its cases. Sizes per case are in the per-case table.",
        "",
        "## Task level (denominator = executed tasks: %d)" % k["tasks"],
        "",
        "| Right stop | Needless stop | Right pass | Missed task (passed, but files changed) | No hook answer (time out · unreadable) |",
        "|---|---|---|---|---|",
        "| %d | %d | %d | %d | %d |" % (k["right_stop"], k["needless_stop"], k["right_pass"], k["missed_task"], k["no_hook_answer"]),
        "",
        "## Human columns",
        "",
        "| Preview right + person refused | Rephrased count |",
        "|---|---|",
        "| not applicable | not applicable |",
        "",
        "A controlled run has no person in it. Filled in by verifier runs.",
        "",
        "## Observer",
        "",
        "Of %s planted changes, the observer missed **%s** (includes changes outside the working folder, same-size overwrite, rename, "
        "size 0 and link change)." % (t["observer"]["planted"], t["observer"]["missed"]),
        "",
        "Observer limits (forms a before/after comparison cannot see — a case of only these forms is «no answer»): %s" % (
            ", ".join(run.get("observer_limits") or []) or "-"),
        "",
        "## Per case",
        "",
        "| Case | Lane | Hook verdict | Count said | Knowledge | Actually changed | Of them outside the folder | Mismatch |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for r in rows:
        a, c = r["actual"], classify(r)
        got = "left out (%s)" % LEFT_OUT_EN.get(a.get("why"), a.get("why")) if c is None else str(_actual_n(a))
        out = "-" if c is None else str((a.get("outside_deleted_n") or 0) + (a.get("outside_overwritten_n") or 0))
        said = ", ".join("%s %s" % (p.get("kind"), p.get("n") if p.get("n") is not None else "no count") for p in r["predicted"]) or "-"
        if r["action"]["lane"] == "F":
            said += " (%s, size %s)" % (r["action"].get("family"), r["action"].get("size"))
        if r["action"]["lane"] in ("I", "F"):
            said += " (as the text reads: %s)" % ("unknown" if r["action"].get("text_n") is None else r["action"]["text_n"])
        lines.append("| %s | %s | %s | %s | %s | %s | %s | %s |" % (r["action"]["case"], r["action"]["lane"], r["judgment"]["verdict"],
                     said, r["knowledge"], got, out, "-" if c is None else (", ".join(c["why"]) or "none")))
    return "\n".join(lines) + "\n"


def main(argv=None):
    ap = argparse.ArgumentParser(prog="olp_bench_report")
    for k in ("--results", "--cases", "--out", "--regressions"):
        ap.add_argument(k, required=True)
    a = ap.parse_args(argv)
    with open(a.results, encoding="utf-8") as f:
        rows = [json.loads(x) for x in f if x.strip()]
    with open(a.cases, encoding="utf-8") as f:
        cases = [json.loads(x) for x in f if x.strip()]
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.write(markdown(rows))
    n = write_regressions(rows, cases, a.regressions)
    print("table -> %s, regression cases %d" % (os.path.basename(a.out), n))
    return 0


if __name__ == "__main__":
    sys.exit(main())
