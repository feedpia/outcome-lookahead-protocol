#!/usr/bin/env bash
# Video preparation. Usage: bash rec_prep.sh <dist_dir> <os_key> <out_dir>
# What it does: creates a fake home + demo project under a short neutral path (no account name), installs the real tgz with npm,
#          and fills the tape templates (@slot@) with this OS's values into <out_dir>. vhs does the recording. No made-up output — all screen text is real execution.
set -eu
DIST="$(cd "$1" && pwd)"; KEY="$2"; OUT="$3"
HERE="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"

if [ "$KEY" = "win32-x64" ]; then
  BASE="/c/olp-demo"; NATIVE_BASE="C:/olp-demo"; SHELL_NAME="powershell"
else
  BASE="/tmp/olp-demo"; NATIVE_BASE="/tmp/olp-demo"; SHELL_NAME="bash"
fi
rm -rf "$BASE"; mkdir -p "$BASE/home" "$BASE/proj/build"
export HOME="$NATIVE_BASE/home" USERPROFILE="$NATIVE_BASE/home"

# demo project: git repo + 3 files in build/ (for the delete card to count)
cd "$BASE/proj"
git init -q
git -c user.email=demo@example.com -c user.name=demo -c commit.gpgsign=false commit -q --allow-empty -m init
echo "console.log(1)" > app.js; echo a > build/a.log; echo b > build/b.log; echo c > build/cache.tmp
git -c user.email=demo@example.com -c user.name=demo add app.js
git -c user.email=demo@example.com -c user.name=demo -c commit.gpgsign=false commit -q -m app
npm init -y >/dev/null 2>&1
npm install --no-audit --no-fund "$DIST/feedpia-olp-$KEY-0.1.3.tgz" "$DIST/feedpia-olp-0.1.3.tgz" >/dev/null 2>&1
cp "$HERE/show-settings.js" "$HERE/hookdemo.js" "$BASE/proj/"
printf 'node_modules/\npackage*.json\nshow-settings.js\nhookdemo.js\n' > .git/info/exclude

fill() { # fill <template> <output>
  sed -e "s|@OS@|$KEY|g" -e "s|@SHELL@|$SHELL_NAME|g" -e "s|@HOME@|$NATIVE_BASE/home|g" \
      -e "s|@PROJ@|$NATIVE_BASE/proj|g" -e "s|@OUT@|$OUT|g" "$1" > "$2"
}
fill "$HERE/scene_hook.tape" "$OUT/scene_hook.tape"

# scene 2 = 2 per language
CCVER="$(claude --version 2>/dev/null | head -1 | awk '{print $1}')"; CCVER="${CCVER:-?}"
CLAUDE_BIN="$(command -v claude 2>/dev/null || echo claude)"
CLAUDE_DIR="$(dirname "$CLAUDE_BIN")"   # the macOS login shell rebuilds PATH and drops the npm global bin → use the full path
DEL_en="Delete the build folder with the Bash tool: rm -rf build"
READ_en="Run git status --short with the Bash tool and show me the output"
DEL_ko="Bash 도구로 build 폴더를 지워 줘: rm -rf build"
READ_ko="Bash 도구로 git status --short 를 실행하고 결과를 보여 줘"
for L in en ko; do
  eval "D=\$DEL_$L; R=\$READ_$L"
  fill "$HERE/scene_claude.tape" "$OUT/scene_claude_$L.tape.tmp"
  sed -e "s|@LANG@|$L|g" -e "s|@CCVER@|$CCVER|g" -e "s|@DEL@|$D|g" -e "s|@READ@|$R|g" -e "s|@CLAUDEDIR@|$CLAUDE_DIR|g" -e "s|@PATH@|$PATH|g" "$OUT/scene_claude_$L.tape.tmp" > "$OUT/scene_claude_$L.tape"
  rm -f "$OUT/scene_claude_$L.tape.tmp"
done

# user settings: only git status skips Claude Code's own permission prompt (so a prompt during a read can be attributed to OLP). The OLP hook entry is in project settings.
mkdir -p "$NATIVE_BASE/home/.claude"
printf '{
  "permissions": { "allow": ["Bash(git status:*)"] }
}
' > "$NATIVE_BASE/home/.claude/settings.json"

# for the claude scene: first-run notice and folder trust already done (a screen a real user passes once — not in the video)
node -e '
  const fs=require("fs"),p=process.argv[1],proj=process.argv[2];
  let real=proj; try { real=fs.realpathSync(proj); } catch (e) {}
  const keys=[...new Set([proj, real, proj.replace(/\//g,"\\"), real.replace(/\//g,"\\")])];   // 맥 /tmp → /private/tmp, Windows 역슬래시 꼴
  const projects={}; for (const k of keys) projects[k]={hasTrustDialogAccepted:true};
  fs.writeFileSync(p, JSON.stringify({hasCompletedOnboarding:true, theme:"dark", projects}, null, 2));
' "$NATIVE_BASE/home/.claude.json" "$NATIVE_BASE/proj"
echo "준비 끝: $OUT (shell=$SHELL_NAME)"
