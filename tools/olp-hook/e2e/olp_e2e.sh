#!/usr/bin/env bash
# End-to-end test of the OLP installer (CI only). Usage: bash olp_e2e.sh <dist_dir> <os_key> [label]   (label or env OLP_E2E_LABEL = name for output, separate from the key that picks the tgz)
# env OLP_E2E_SLOW=1 : slow-start mode (a card on a read-only command is not a FAIL, it is recorded as FACT)
# dist_dir = npm pack output (tgz). Like a real user: npm install -> npx olp install -> run the command from settings.json as written.
# Every file is created inside a temp folder only (fake home + a path with spaces and Korean characters).
set -u
DIST="${1:?usage: olp_e2e.sh <dist_dir> <os_key>}"
KEY="${2:?usage: olp_e2e.sh <dist_dir> <os_key>}"
LABEL="${3:-${OLP_E2E_LABEL:-$KEY}}"   # for output
SLOW="${OLP_E2E_SLOW:-0}"
CARD_LIMIT_MS=10000   # the delete card must appear within this (FAIL if over)
DIST="$(cd "$DIST" && pwd)"
VER="0.1.1"

PASS=0; FAIL=0; SKIP=0; FACT=0
ok()   { echo "PASS  $1"; PASS=$((PASS+1)); }
bad()  { echo "FAIL  $1"; FAIL=$((FAIL+1)); }
skip() { echo "SKIP  $1  -- $2"; SKIP=$((SKIP+1)); }
fact() { echo "FACT  $1"; FACT=$((FACT+1)); }
check() { # check <name> <command...> : PASS if the command succeeds
  local name="$1"; shift
  if "$@"; then ok "$name"; else bad "$name"; fi
}

# Windows (Git Bash) must give native programs C:/-style paths. Elsewhere pwd as is.
native_pwd() { pwd -W 2>/dev/null || pwd; }

ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
mkdir -p "$ROOT/홈 폴더" "$ROOT/작업 폴더"
FAKE_HOME="$(cd "$ROOT/홈 폴더" && native_pwd)"
export HOME="$FAKE_HOME" USERPROFILE="$FAKE_HOME"   # USERPROFILE on Windows, HOME elsewhere
export npm_config_update_notifier=false
export npm_config_cache="$FAKE_HOME/npm-cache"   # Windows npm cache follows LOCALAPPDATA, not HOME: keep it inside the fake home
EXE_NAME="olp-hook"; [ "$KEY" = "win32-x64" ] && EXE_NAME="olp-hook.exe"
INSTALLED="$FAKE_HOME/olp/$EXE_NAME"

G() { git -c user.email=e2e@example.com -c user.name=e2e -c commit.gpgsign=false -c core.autocrlf=false "$@"; }

# fake project: git repo + committed files + a build/ folder
make_project() { # make_project <name> -> prints the path
  local d="$ROOT/작업 폴더/$1"
  mkdir -p "$d/build/sub"
  ( cd "$d" && G init -q \
    && echo "console.log(1)" > app.js && echo "# x" > README.md \
    && echo a > build/a.txt && echo b > build/b.txt && echo c > build/sub/c.txt \
    && G add -A && G commit -q -m init ) >/dev/null 2>&1
  echo "$d"
}

# count our entries / read the command in settings.json (parsed with node, the command is never rebuilt)
settings_info() { # settings_info <file> <count|post|cmd|other|key> [key name]
  node -e '
    const s = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
    const mode = process.argv[2];
    const flat = (ev) => ((s.hooks && s.hooks[ev]) || []).flatMap(e => e.hooks || []);
    const mine = (h) => /olp-hook(\.exe)?"?$/.test(h.command || "");
    const ours = flat("PreToolUse").filter(mine);
    if (mode === "count") console.log(ours.length);
    else if (mode === "cmd") console.log(ours.length ? ours[0].command : "");
    else if (mode === "other") console.log(flat("PreToolUse").length - ours.length);
    else if (mode === "post") console.log(flat("PostToolUse").filter(mine).length);
    else if (mode === "key") console.log(s[process.argv[3]] === undefined ? "missing" : JSON.stringify(s[process.argv[3]]));
  ' "$1" "$2" "${3:-}" 2>/dev/null
}

# Run the hook like Claude Code: the registered command string through a shell (bash -c), PreToolUse JSON on stdin, cwd = project
# result files: $OUT (stdout) $OUT.err $OUT.code
now_ms() { node -e 'console.log(Date.now())'; }   # date +%s%N does not work on macOS
PREFIX=()   # command prefix (array: safe for paths with spaces and Korean). e.g. (env OLP_HOOK_PS_PARSER=off)
run_hook() { # run_hook <project> <command string> <tool_name> <command for the tool>
  local proj="$1" cmdstr="$2" tool="$3" tcmd="$4" pw json t0
  pw="$(cd "$proj" && native_pwd)"
  json="$(node -e 'process.stdout.write(JSON.stringify({session_id:"s",transcript_path:"t",cwd:process.argv[1],hook_event_name:"PreToolUse",tool_name:process.argv[2],tool_input:{command:process.argv[3]},permission_mode:"default"}))' "$pw" "$tool" "$tcmd")"
  OUT="$ROOT/hook_out_$RANDOM"
  t0="$(now_ms)"
  # shell that calls the hook: bash by default (same as Claude Code). OLP_E2E_HOOKSHELL=powershell|pwsh uses that shell
  local HOOKRUN
  if [ -n "${OLP_E2E_HOOKSHELL:-}" ]; then HOOKRUN="$OLP_E2E_HOOKSHELL -NoProfile -NonInteractive -Command"; cmdstr="& $cmdstr"; else HOOKRUN="bash -c"; fi
  ( cd "$proj" && printf '%s' "$json" | ${PREFIX[@]+"${PREFIX[@]}"} $HOOKRUN "$cmdstr" >"$OUT" 2>"$OUT.err" )
  echo $? > "$OUT.code"
  echo $(( $(now_ms) - t0 )) > "$OUT.ms"   # time taken by the hook call (ms)
}
decision() { node -e 'try{console.log(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).hookSpecificOutput.permissionDecision)}catch(e){console.log("none")}' "$OUT"; }
reason_has() { node -e 'try{process.exit(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).hookSpecificOutput.permissionDecisionReason.includes(process.argv[2])?0:1)}catch(e){process.exit(1)}' "$OUT" "$1"; }
is_card() { [ "$(cat "$OUT.code")" = "0" ] && [ "$(decision)" = "ask" ]; }
card_within_10s() { is_card && [ "$(cat "$OUT.ms")" -lt "$CARD_LIMIT_MS" ]; }
is_pass() { [ "$(cat "$OUT.code")" = "0" ] && [ ! -s "$OUT" ]; }
read_check() { # read_check <name> : PASS only if allowed by default. In slow mode a card is a FACT (safe side)
  if [ "$SLOW" = "1" ] && ! is_pass && is_card; then
    fact "${1%% *} 읽기: card ($(cat "$OUT.ms")ms) — 느린 기동에선 묻기가 늘 수 있음(안전 쪽)"
  elif is_pass; then
    if [ "$SLOW" = "1" ]; then fact "${1%% *} 읽기: pass ($(cat "$OUT.ms")ms) — 느린 기동에선 묻기가 늘 수 있음(안전 쪽)"; else ok "$1"; fi
  else bad "$1"; fi
}
no_panic() { ! grep -q panicked "$1"; }
has_text() { [ -s "$1" ]; }

echo "== E2E $LABEL  (key: $KEY, slow: $SLOW, root: $ROOT)"
echo "== 가짜 홈: $FAKE_HOME"
npm_cache_inside() { case "$(npm config get cache 2>/dev/null | tr '\' /)" in "$(printf %s "$FAKE_HOME" | tr '\' /)"/*) return 0;; *) return 1;; esac; }
check "0 npm 캐시가 가짜 홈 안" npm_cache_inside

# ---------- L1: real install with npm ----------
P1="$(make_project proj)"
( cd "$P1" && npm init -y >/dev/null 2>&1 )
echo "-- npm install (로컬 tgz 2개)"
( cd "$P1" && npm install --no-audit --no-fund --fetch-retries=0 \
    "$DIST/feedpia-olp-$KEY-$VER.tgz" "$DIST/feedpia-olp-$VER.tgz" 2>&1 | tail -5 )
check "L1 npm install 성공(node_modules/@feedpia/olp 있음)" test -d "$P1/node_modules/@feedpia/olp"
echo "-- npx --no-install olp install"
( cd "$P1" && npx --no-install olp install 2>&1 < /dev/null ); INST_RC=$?
check "L1 olp install 종료 0" test "$INST_RC" = "0"
S1="$P1/.claude/settings.json"
check "L1 .claude/settings.json 생김" test -f "$S1"
check "L1 PreToolUse 우리 칸 정확히 1개" test "$(settings_info "$S1" count)" = "1"
check "L1 PostToolUse 우리 칸 1개" test "$(settings_info "$S1" post)" = "1"
check "L1 실행 파일이 가짜 홈/olp 에 복사됨" test -f "$INSTALLED"
CMD="$(settings_info "$S1" cmd)"
echo "-- 등록된 명령: $CMD"
# Does the binary the registered command points at exist (installing from a renamed file used to exit 0 without this file → every shell command blocked)
cmd_file_exists() { node -e 'const c=process.argv[1].replace(/^"|"$/g,"");process.exit(require("fs").existsSync(c)?0:1)' "$1"; }
check "L1 등록된 명령의 실행 파일이 실제로 있음" cmd_file_exists "$CMD"

# 1. run the registered command as is from a path with spaces and Korean characters
run_hook "$P1" "$CMD" Bash "rm -rf build"
check "1 삭제 -> 카드(ask) 10초 안, 공백·한글 경로 ($(cat "$OUT.ms")ms)" card_within_10s
Q_CARD_DELETE=0; card_within_10s && reason_has "delete files" && Q_CARD_DELETE=1
check "1 삭제 카드 글자에 'delete files'" test "$Q_CARD_DELETE" = "1"
# overwrite card: redirecting into a committed file
run_hook "$P1" "$CMD" Bash "echo x > README.md"
Q_CARD_OVERWRITE=0; card_within_10s && reason_has "overwrite files" && Q_CARD_OVERWRITE=1
check "1 덮어쓰기 -> 카드(ask) 10초 안, 글자에 'overwrite files' ($(cat "$OUT.ms")ms)" test "$Q_CARD_OVERWRITE" = "1"
check "1 시험이 README.md 를 바꾸지 않음(셈만)" test "$(cat "$P1/README.md")" = "# x"
run_hook "$P1" "$CMD" Bash "git status --short"
read_check "1 읽기 -> 빈 출력·종료 0(통과)"
check "1 .olp/hook-log.jsonl 에 줄이 쌓임(훅이 진짜 돎)" test -s "$P1/.olp/hook-log.jsonl"
check "1 시험이 build/ 를 지우지 않음(셈만)" test -f "$P1/build/a.txt"

# ---------- 2. project that already has someone else's hook ----------
P2="$(make_project other)"
mkdir -p "$P2/.claude"
cat > "$P2/.claude/settings.json" <<'JSON'
{
  "theme": "dark",
  "hooks": {
    "PreToolUse": [
      { "matcher": "Edit", "hooks": [ { "type": "command", "command": "echo other-hook" } ] }
    ]
  }
}
JSON
( cd "$P2" && "$INSTALLED" install </dev/null >/dev/null 2>&1 ); RC=$?
S2="$P2/.claude/settings.json"
check "2 남의 훅 있는 설정에 install 성공" test "$RC" = "0"
check "2 우리 칸 1개" test "$(settings_info "$S2" count)" = "1"
check "2 남의 훅 1개 그대로" test "$(settings_info "$S2" other)" = "1"
check "2 남의 최상위 키(theme) 그대로" test "$(settings_info "$S2" key theme)" = '"dark"'

# ---------- 3. broken settings.json ----------
P3="$(make_project broken)"
mkdir -p "$P3/.claude"
printf '{ not json' > "$P3/.claude/settings.json"
cp "$P3/.claude/settings.json" "$ROOT/broken.before"
( cd "$P3" && "$INSTALLED" install </dev/null >"$ROOT/broken.out" 2>&1 ); RC=$?
check "3 깨진 JSON -> install 종료 코드 0 아님" test "$RC" != "0"
check "3 깨진 파일 바이트 그대로" cmp -s "$ROOT/broken.before" "$P3/.claude/settings.json"
check "3 panic 문구 없음" no_panic "$ROOT/broken.out"

# ---------- 4. install twice ----------
cp "$S2" "$ROOT/s2.before"
( cd "$P2" && "$INSTALLED" install </dev/null >/dev/null 2>&1 ); RC=$?
check "4 두 번째 install 종료 0" test "$RC" = "0"
check "4 우리 칸 여전히 1개" test "$(settings_info "$S2" count)" = "1"
check "4 설정 파일 바이트 동일" cmp -s "$ROOT/s2.before" "$S2"
cp "$S1" "$ROOT/s1.before"
( cd "$P1" && npx --no-install olp install </dev/null >/dev/null 2>&1 )
check "4 npx 로 다시 설치해도 바이트 동일" cmp -s "$ROOT/s1.before" "$S1"

# ---------- 5. uninstall ----------
( cd "$P2" && "$INSTALLED" uninstall </dev/null >/dev/null 2>&1 ); RC=$?
check "5 uninstall 종료 0" test "$RC" = "0"
check "5 우리 PreToolUse 칸 사라짐" test "$(settings_info "$S2" count)" = "0"
check "5 우리 PostToolUse 칸 사라짐" test "$(settings_info "$S2" post)" = "0"
check "5 남의 훅 남아 있음" test "$(settings_info "$S2" other)" = "1"
check "5 남의 키(theme) 남아 있음" test "$(settings_info "$S2" key theme)" = '"dark"'

# ---------- 6. no network (Linux only) ----------
# Runners block user namespaces (unshare -rn), so create a network namespace with sudo and drop back to the original user with setpriv.
if [ "$KEY" != "linux-x64" ]; then
  skip "6 네트워크 차단" "리눅스 전용(이름공간 필요)"
elif ! sudo -n true >/dev/null 2>&1 || ! command -v setpriv >/dev/null 2>&1 || ! sudo -n unshare -n true >/dev/null 2>&1; then
  skip "6 네트워크 차단" "sudo·unshare -n·setpriv 사용 불가"
else
  PREFIX=(sudo -n unshare -n setpriv --reuid="$(id -u)" --regid="$(id -g)" --clear-groups env HOME="$FAKE_HOME" USERPROFILE="$FAKE_HOME" PATH="$PATH")
  # first confirm it is really cut off (reaching an outside address must fail)
  if "${PREFIX[@]}" bash -c 'exec 3<>/dev/tcp/1.1.1.1/443' >/dev/null 2>&1; then
    bad "6 네트워크가 정말 끊겼나(바깥 연결이 됨)"
  else
    ok "6 네트워크가 정말 끊김(바깥 연결 실패)"
  fi
  run_hook "$P1" "$CMD" Bash "rm -rf build"
  check "6 네트워크 없이 삭제 -> 카드 10초 안 ($(cat "$OUT.ms")ms)" card_within_10s
  run_hook "$P1" "$CMD" Bash "git status --short"
  read_check "6 네트워크 없이 읽기 -> 통과"
  PREFIX=()
fi

# ---------- 7. PowerShell parser off (Windows only) ----------
if [ "$KEY" != "win32-x64" ]; then
  skip "7 PowerShell 해석기 끔" "윈도우 전용"
else
  PREFIX=(env OLP_HOOK_PS_PARSER=off)
  run_hook "$P1" "$CMD" PowerShell "Remove-Item -Recurse -Force build"
  check "7 해석기 끄고 PowerShell 삭제 -> 카드 10초 안 ($(cat "$OUT.ms")ms)" card_within_10s
  PREFIX=()
fi

# ---------- 9. PowerShell 7-only syntax (Windows only) ----------
# && exists only in PowerShell 7. The hook parser is Windows PowerShell 5.1 and cannot read it -> must fall back to asking.
if [ "$KEY" != "win32-x64" ]; then
  skip "9 PowerShell 7 전용 문법 삭제" "윈도우 전용"
else
  run_hook "$P1" "$CMD" PowerShell "Test-Path build && Remove-Item -Recurse -Force build"
  check "9 PowerShell 7 전용(&&) 삭제 -> 카드 10초 안 ($(cat "$OUT.ms")ms)" card_within_10s
fi

# ---------- 10. install from the GitHub release archive (people who download without npm) ----------
# release archive = olp-hook-<os>-<arch>.zip|.tar.gz with olp-hook(.exe) inside. Unpack → ./olp-hook install → registered binary exists + delete = card.
case "$KEY" in
  win32-x64) RN=windows-x64; RX=zip ;; linux-x64) RN=linux-x64; RX=tar.gz ;; linux-arm64) RN=linux-arm64; RX=tar.gz ;;
  darwin-arm64) RN=macos-arm64; RX=tar.gz ;; darwin-x64) RN=macos-x64; RX=tar.gz ;; *) RN=""; RX="" ;;
esac
ARC="$DIST/olp-hook-$RN.$RX"
if [ -z "$RN" ] || [ ! -f "$ARC" ]; then
  skip "10 릴리스 묶음 설치" "묶음 파일 없음($ARC)"
else
  RH="$ROOT/릴리스 홈"; RD="$ROOT/내려받기"; mkdir -p "$RH" "$RD"
  RH_N="$(cd "$RH" && native_pwd)"
  if [ "$RX" = zip ]; then powershell.exe -NoProfile -NonInteractive -Command "Expand-Archive -LiteralPath '$(cd "$(dirname "$ARC")" && native_pwd)/$(basename "$ARC")' -DestinationPath '$(cd "$RD" && native_pwd)' -Force" >/dev/null
  else tar -xzf "$ARC" -C "$RD"; fi
  P5="$(make_project release)"
  ( cd "$P5" && HOME="$RH_N" USERPROFILE="$RH_N" "$RD/$EXE_NAME" install </dev/null >"$ROOT/rel.out" 2>&1 ); RC=$?
  check "10 묶음을 풀어 install 종료 0" test "$RC" = "0"
  RCMD="$(settings_info "$P5/.claude/settings.json" cmd)"
  check "10 등록된 명령의 실행 파일이 실제로 있음" cmd_file_exists "$RCMD"
  HOME="$RH_N" USERPROFILE="$RH_N" run_hook "$P5" "$RCMD" Bash "rm -rf build"
  check "10 묶음 설치에서 지우기 -> 카드 10초 안 ($(cat "$OUT.ms")ms)" card_within_10s
fi

# ---------- 8. unusual home ----------
# (a) project = home, no --user -> refused, settings not written
( cd "$FAKE_HOME" && "$INSTALLED" install </dev/null >"$ROOT/home.out" 2>&1 ); RC=$?
check "8a 홈 폴더에서 install -> 거절(종료 코드 0 아님)" test "$RC" != "0"
check "8a 홈에 .claude/settings.json 안 써짐" test ! -e "$FAKE_HOME/.claude/settings.json"
# (b) no HOME or USERPROFILE -> clean failure
P4="$(make_project nohome)"
( cd "$P4" && env -u HOME -u USERPROFILE "$INSTALLED" install </dev/null >"$ROOT/nohome.out" 2>&1 ); RC=$?
check "8b 홈 변수 없음 -> 종료 코드 0 아님" test "$RC" != "0"
check "8b 메시지가 있음" has_text "$ROOT/nohome.out"
check "8b panic 문구 없음" no_panic "$ROOT/nohome.out"
check "8b 설정 안 써짐" test ! -e "$P4/.claude/settings.json"

# ---------- 11. uninstall the main project (npx), for the receipt ----------
Q_HOOKS=$(( $(settings_info "$S1" count) + $(settings_info "$S1" post) ))
( cd "$P1" && npx --no-install olp uninstall </dev/null >/dev/null 2>&1 ); Q_UNINSTALL=$?
check "11 npx olp uninstall 종료 0" test "$Q_UNINSTALL" = "0"
Q_HOOKS_AFTER=$(( $(settings_info "$S1" count) + $(settings_info "$S1" post) ))
check "11 uninstall 뒤 우리 칸 0개" test "$Q_HOOKS_AFTER" = "0"

# ---------- 12. upgrade over the published 0.1.1, then undo ----------
# A person who installed 0.1.1 from npm installs this build on top: still one entry of ours, the program is the new one,
# someone else's hook and keys untouched, and uninstall gives back the settings file exactly as it was before 0.1.1.
OLD="0.1.1"
P6="$(make_project upgrade)"
mkdir -p "$P6/.claude"
cat > "$P6/.claude/settings.json" <<'JSON'
{
  "theme": "dark",
  "hooks": {
    "PreToolUse": [
      { "matcher": "Edit", "hooks": [ { "type": "command", "command": "echo other-hook" } ] }
    ]
  }
}
JSON
S6="$P6/.claude/settings.json"
cp "$S6" "$ROOT/s6.orig"
( cd "$P6" && npm init -y >/dev/null 2>&1 )
( cd "$P6" && npm install --no-audit --no-fund "@feedpia/olp@$OLD" 2>&1 | tail -3 ); RC=$?
check "12 npm 에서 $OLD 설치" test -d "$P6/node_modules/@feedpia/olp"
( cd "$P6" && npx --no-install olp install </dev/null >/dev/null 2>&1 ); RC=$?
check "12 $OLD install 종료 0" test "$RC" = "0"
check "12 $OLD 뒤 우리 칸 1개" test "$(settings_info "$S6" count)" = "1"
( cd "$P6" && npm install --no-audit --no-fund --fetch-retries=0 \
    "$DIST/feedpia-olp-$KEY-$VER.tgz" "$DIST/feedpia-olp-$VER.tgz" 2>&1 | tail -3 )
( cd "$P6" && npx --no-install olp install </dev/null >/dev/null 2>&1 ); RC=$?
check "12 이 판으로 다시 install 종료 0" test "$RC" = "0"
check "12 업그레이드 뒤 우리 PreToolUse 칸 1개" test "$(settings_info "$S6" count)" = "1"
check "12 업그레이드 뒤 우리 PostToolUse 칸 1개" test "$(settings_info "$S6" post)" = "1"
check "12 남의 훅 1개 그대로" test "$(settings_info "$S6" other)" = "1"
check "12 남의 키(theme) 그대로" test "$(settings_info "$S6" key theme)" = '"dark"'
new_bin_is_installed() { tar -xOzf "$DIST/feedpia-olp-$KEY-$VER.tgz" "package/bin/$EXE_NAME" | cmp -s - "$INSTALLED"; }
check "12 설치된 실행 파일 = 이 판의 실행 파일" new_bin_is_installed
( cd "$P6" && npx --no-install olp uninstall </dev/null >/dev/null 2>&1 ); RC=$?
check "12 uninstall 종료 0" test "$RC" = "0"
# Uninstall rewrites the file in standard formatting and keeps the empty event lists install created (install.rs remove_where),
# so the bytes differ from the original even after a single install and uninstall (also in 0.1.1). Check the meaning instead:
# same JSON once empty hook lists are dropped, and record whether the bytes are the same.
same_meaning() { node -e '
  const fs = require("fs");
  const norm = (t) => { const s = JSON.parse(t);
    if (s.hooks) { for (const k of Object.keys(s.hooks)) if (Array.isArray(s.hooks[k]) && s.hooks[k].length === 0) delete s.hooks[k];
                   if (Object.keys(s.hooks).length === 0) delete s.hooks; }
    return JSON.stringify(s); };
  process.exit(norm(fs.readFileSync(process.argv[1], "utf8")) === norm(fs.readFileSync(process.argv[2], "utf8")) ? 0 : 1);
' "$1" "$2"; }
check "12 uninstall 뒤 설정 = $OLD 설치 전과 같은 뜻(빈 훅 목록만 남음)" same_meaning "$ROOT/s6.orig" "$S6"
if cmp -s "$ROOT/s6.orig" "$S6"; then fact "12 uninstall 뒤 바이트도 원본과 같음"; else fact "12 uninstall 뒤 바이트는 원본과 다름(표준 꼴로 다시 씀 + 빈 이벤트 목록)"; fi

echo "E2E $LABEL: $PASS PASS, $FAIL FAIL, $SKIP SKIP, $FACT FACT"

# Receipt for the tag gate (DESIGN_RELEASE-QUAL-012 §2), only when OLP_E2E_QUAL names a file. Numbers are what this run saw.
# hooks = our PreToolUse + PostToolUse entries after install (counted just before uninstall; installs since then left it unchanged, see 4).
if [ -n "${OLP_E2E_QUAL:-}" ]; then
  node -e '
    const [out, key, inst, hooks, del, ow, un, after, fail, exe] = process.argv.slice(1);
    const fs = require("fs"), crypto = require("crypto");
    const counts = { install: +inst, hooks: +hooks, card_delete: +del, card_overwrite: +ow, uninstall: +un, hooks_after: +after };
    const meets = counts.install === 0 && counts.hooks === 2 && counts.card_delete === 1 && counts.card_overwrite === 1 && counts.uninstall === 0 && counts.hooks_after === 0;
    let sha = null; try { sha = crypto.createHash("sha256").update(fs.readFileSync(exe)).digest("hex"); } catch (e) {}
    const q = { kind: "e2e", os: key, head_sha: process.env.GITHUB_SHA || null, run_id: process.env.GITHUB_RUN_ID ? +process.env.GITHUB_RUN_ID : null,
      ok: meets && +fail === 0, counts, hook_sha256: sha, made_at: new Date().toISOString().replace(/\.\d+Z$/, "Z") };
    fs.writeFileSync(out, JSON.stringify(q, null, 2) + "\n");
    console.log("qual.json: " + JSON.stringify(q));
  ' "$OLP_E2E_QUAL" "$KEY" "$INST_RC" "$Q_HOOKS" "$Q_CARD_DELETE" "$Q_CARD_OVERWRITE" "$Q_UNINSTALL" "$Q_HOOKS_AFTER" "$FAIL" "$P1/node_modules/@feedpia/olp-$KEY/bin/$EXE_NAME"
fi
[ "$FAIL" = "0" ]
