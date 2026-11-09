# olp-hook — 지우기·덮어쓰기 전에, 무엇이 사라지는지 먼저 봅니다

**0.1.1**

OLP(Outcome Lookahead Protocol)는 에이전트의 행동이 낳을 결과를 내다보는 방법을 정한 공개 규격입니다.

0.1.1: Claude Code 훅 — 지우기 직전엔 어떤 파일이 몇 개인지, 덮어쓰기 직전엔 몇 개인지 카드로 보여 줍니다.

`rm -rf build` 가 실행되기 전, Claude Code 확인 창에서:

```text
[OLP] Before this runs: delete files — 3 files (preview: counted on this device, read-only) | Files: build/app.log, build/cache.tmp, build/notes.txt | before engine check · no receipt
```

```
npx @feedpia/olp install
```

### Guide video (2 min)

English subtitles:

https://github.com/user-attachments/assets/90489855-3ca4-4792-8889-cf647e7c7fd5

Korean subtitles:

https://github.com/user-attachments/assets/3d8dd4bc-a772-435c-88a5-6efbc3443e76

Recorded on 0.1.0. The card behaves the same in 0.1.1.

**되돌릴 수 없는 승인 앞에서, 무엇이 바뀌는지 먼저 보여 드립니다.**

AI 에이전트가 되돌릴 수 없는 일을 하기 전에, OLP가 무엇이 사라지는지 셉니다.

## 왜 필요한가

명령 하나가 보기보다 훨씬 많이 바꿀 수 있습니다. 예를 들어 두 표를 엉뚱한 열로 합치면 50줄이 172줄로 불어날 수 있고, 나중에야 알게 됩니다. 이런 일은 명령이 돌기 전, 바뀌는 크기를 아직 볼 수 있을 때 잡는 것이 가장 쌉니다. 지금 훅은 이 기기의 파일과 git 기록을 셉니다. 데이터베이스 행은 당신이 준 숫자만 보여 줍니다.

OLP 는 당신의 에이전트나 그 확인 창을 바꾸지 않습니다. 그 앞에 한 단계를 덧붙입니다.

## 설치와 되돌리기

Claude Code 를 쓰는 프로젝트 폴더에서(Node.js 18 이상):

```
npx @feedpia/olp install
```

- `olp-hook` 을 홈 폴더의 `olp` 로 복사하고(Windows 는 `C:\Users\<당신 이름>\olp`), `.claude/settings.json` 의 `PreToolUse`·`PostToolUse` 에 `olp-hook` 칸을 하나씩 더합니다. 그 뒤 훅은 Node 를 거치지 않고 이 복사본을 바로 실행합니다.
- 다른 칸과 키 순서는 그대로 둡니다. 두 번 돌려도 더 늘지 않습니다. 파일이 올바른 JSON 이 아니면 아무것도 바꾸지 않습니다.
- 쓰기 전에 원래 파일을 홈 폴더의 `olp/backup` 에 복사해 둡니다(프로젝트 밖).
- 마지막에 한 장으로 보여 줍니다: 더한 칸, OLP 가 하는 일, 당신의 «늘 허락» 권한 규칙(사용자·프로젝트·로컬 설정) 중 지우기·덮어쓰기 명령에 닿는 것이 몇 개인지, 되돌리는 법. 그 규칙은 읽기만 하고 바꾸지 않습니다.
- `--dry-run` 은 바꿀 내용만 보여 주고 아무것도 바꾸지 않습니다. `--user` 는 사용자 설정(`~/.claude/settings.json`)을 고칩니다. `--replace` 는 이미 있는 `olp-hook` 칸을 새 복사본으로 바꿉니다.
- 홈 폴더에서 `--user` 없이 치면 아무것도 바꾸지 않고 멈춥니다. 그 자리의 설정 파일은 모든 프로젝트에 걸리기 때문입니다. 프로젝트 폴더 안에서 치거나, 정말 모든 프로젝트에 걸려면 `--user` 를 붙이세요.
- `.olp/hook-config.json` 의 `{"decision": true}`(실험, 기본 꺼짐): 지우는 파일이 2개 이상이고 그중 git 에 없는 파일이 있으면, 훅이 그 지우기를 한 번 멈추고 에이전트에게 물어볼 선택지 3개(취소가 첫 칸, 그다음 일부 남기기·전부 지우기)를 줍니다. 고른 것은 에이전트의 다음 지시이고, 다음 지우기에는 보통 카드와 허락 창이 뜹니다.
- 업데이트: 같은 설치 명령을 다시 실행합니다. 0.1.1은 스스로 업데이트하지 않습니다.
- 되돌리기: `npx @feedpia/olp uninstall` 은 `olp-hook` 줄만 뺍니다. `olp` 폴더의 프로그램은 남겨 두니, 필요 없으면 그 폴더를 지우세요.
- 복사본은 패키지 안의 파일과 한 바이트씩 비교합니다. 이것은 복사가 깨지지 않았다는 뜻일 뿐, 패키지가 우리 것이라는 뜻은 아닙니다. 그것은 릴리스의 `SHA256SUMS` 로 확인하세요.
- 릴리스에서 받았다면 — 묶음 `olp-hook-<os>-<arch>.zip`/`.tar.gz` 를 프로젝트 밖(예: 내려받기 폴더)에 푸세요(안의 이름 `olp-hook`, Windows 는 `olp-hook.exe` — 바꾸지 마세요). macOS 에서 브라우저로 받으면 서명 없는 판이라 막히니 먼저 `xattr -d com.apple.quarantine <경로>/olp-hook` 을 치세요. 그다음 프로젝트 폴더 안에서 그 파일을 `install` 로 실행합니다: `~/Downloads/olp-hook install`(Windows: `~\Downloads\olp-hook.exe install`). 자기 자신을 `~/olp/` 로 복사하고 `.claude/settings.json` 에 전체 경로를 적습니다. 이름을 바꿨으면 install 은 아무것도 바꾸지 않고 한 줄 이유를 보이며 멈춥니다.

판: Windows x64, macOS(Apple 칩, Intel), Linux x64, Linux arm64. 그 밖의 시스템에서는 아직 판이 없다고 말하고 멈춥니다.

## 작동 방식

**엔진 확인 전 · 영수증 없음.** 이 훅은 당신 기기에서만 돌고, 훅 스스로는 네트워크에 연결하지 않습니다. 따로 켜는 하루 사용 숫자(숫자만)는 아래에 적었고, 기본은 꺼져 있으며 예라고 답한 사람만 보냅니다.

설정은 Claude Code 설정 파일(`.claude/settings.json`의 `hooks` 항목)에 한 줄입니다. 같은 실행 파일이 도구 호출 앞뒤에서 돕니다.

```json
"hooks": { "PreToolUse": [ { "matcher": "Bash|PowerShell|mcp__.*", "hooks": [ { "type": "command", "command": "olp-hook", "timeout": 10, "onFailure": "block" } ] } ], "PostToolUse": [ { "matcher": "Bash|PowerShell|mcp__.*", "hooks": [ { "type": "command", "command": "olp-hook", "timeout": 10 } ] } ] }
```

이번 판은 셸 명령(`Bash`, `PowerShell`)과 MCP 도구만 봅니다. 파일 편집(Edit/Write)은 걸지 않습니다. 파일 편집은 Claude Code가 자체 체크포인트로 따로 기록하고, 이 훅은 그에 대해 아무것도 말하지 않습니다.

- **첫 주에는 자주 물어볼 수 있습니다.** 아직 모르는 명령이나 도구는 쓰기 작업으로 봅니다. 그때마다 Claude Code 확인 창에 카드가 뜨고, 분류하는 명령 한 줄이 함께 나옵니다. 한 번 답하면 이 프로젝트에서는 그 답을 기억합니다.
- **고르지 않은 일도 이 기기에 남습니다.** 카드, 세지 못한 호출, 카드 뒤에 실행되지 않은 호출이 프로젝트 폴더의 `.olp/hook-log.jsonl`에 적힙니다.
- **엔진 확인 전입니다.** 쓰기와 돈 이동은 늘 사람에게 넘어갑니다. 여기 있는 어떤 글도 써도 괜찮다는 뜻이 아닙니다.

---

## 하는 일

도구 호출마다, 실행 전에 세 가지 중 하나로 나눕니다: `read_only`(읽기만), `writes_other`(쓰기), `moves_money`(돈 이동).

| 호출 | 결과 |
|---|---|
| 기본 읽기 도구(`Read`, `Glob`, `Grep`, `LS`, `NotebookRead`, `TodoWrite`) | 그냥 지나감 |
| 정해진 읽기 목록의 셸 명령(`ls`, `cat`, `head`, `tail`, `grep`, `rg`, 쓰기 옵션 없는 `find`, `git status/log/diff/show/ls-files/rev-parse/blame`, `echo`, `pwd`, `wc` 등) | 그냥 지나감 |
| 셸 명령의 위험한 여섯 꼴: 파일 지우기, 파일 덮어쓰기, git 강제 푸시(원격 브랜치 지우기 포함), `git reset --hard`, 데이터베이스 행 지우기, 데이터베이스 행 바꾸기 | 확인 창에 카드. 명령 묶음에 준 답만으로는 지나가지 않습니다. |
| 그 밖의 것 | 쓰기로 보고 확인 창에 카드, 분류하는 명령 한 줄과 함께 |

카드가 어디에 뜨는지는 Claude Code 권한 모드에 따라 다릅니다.

- `default`, `plan`, `acceptEdits`, `auto`(데스크톱 앱 기본): Claude Code가 자기 확인 창에 카드를 보여 줍니다. 직접 고릅니다.
- `bypassPermissions`, `dontAsk`, 그 밖의 모드나 모드가 없을 때: 확인 창을 건너뛸 수 있어서, 훅이 호출을 막고 카드를 보여 줍니다. 이 모드에서 OLP 는 멈추기만 합니다. 진행하려면 묻는 모드로 바꾸고(Shift+Tab) 허락 창에서 답하세요.
- 훅 오류·시간 초과·멈춤은 언제나 막습니다.

허락 창 안에서는 카드가 한 줄로 나오고, OLP가 낸 글임을 알 수 있게 늘 `[OLP]`로 시작합니다:

```
[OLP] Before this runs: delete files — 3 files (preview: counted on this device, read-only) | Files: build/a.txt, build/b.txt, build/c.log | before engine check · no receipt
```

카드 전문은 허락 창 옆에 나오는 글이고, 이렇게 생겼습니다(실제 출력, 영어 그대로):

```
OLP hook: check before it runs — before engine check · no receipt
What it does: delete files — 3 files (preview: counted on this device, read-only)
Files: build/a.txt, build/b.txt, build/c.log
Undo: not confirmed — no recovery location declared
Why: WRITE_IRREVERSIBLE, WRITE_RECOVERY_UNKNOWN
```

뜻: 실행 전 확인(엔진 확인 전 · 영수증 없음) / 하는 일: 파일 지우기 — 3개(미리 본 값: 이 기기에서 읽기만 해서 셈) / 파일: 이름 최대 3개, 그보다 많으면 `and N more`(외 N개) / 되돌리기: 확인 못 함 — 선언된 복구 위치 없음. 숫자는 실행 전에 미리 본 값이지 약속이 아닙니다. 읽기만 하는 명령에는 카드가 없습니다.

확인 창의 Always allow(항상 허용)를 고르면, 같은 명령에서는 다음부터 이 카드가 안 뜰 수 있습니다(확인 중).

## 결제 도구 앞에서

결제 도구는 보통 MCP(`mcp__...`)로 들어오므로 위 matcher 가 이미 봅니다. 결제 도구를 쓰는 프로젝트 폴더마다, 결제 도구 하나하나를 `moves_money`(돈 이동)로 한 번 분류하세요. 그 뒤로는 부를 때마다 실행 전에 카드가 뜹니다. 조용한 모드도 지나가지 않습니다.

당신의 결제 MCP 서버가 보여 주는 도구 이름을 쓰세요. 아래 이름은 자리표입니다.

**결제 서버가 Stripe 라면**(지원 나라는 Stripe 공식 목록 참고):

```
olp-hook classify --tool mcp__<stripe-server>__<payment-tool> moves_money
```

**결제 서버가 카카오페이라면**(한국 결제 서비스):

```
olp-hook classify --tool mcp__<kakaopay-server>__<payment-tool> moves_money
```

**결제 서버가 토스페이먼츠라면**(한국 결제 서비스):

```
olp-hook classify --tool mcp__<tosspayments-server>__<payment-tool> moves_money
```

카드는 이렇게 생겼습니다(실제 출력, 시험용 폴더, 도구 `mcp__payments__create_payment`, 영어 그대로):

```
OLP hook: check before it runs — before engine check · no receipt
What it does: moves money (classified by you) — count not checked
Amount: check it in the tool input before you answer (the hook does not read amounts)
Undo: not confirmed — no recovery location declared
Why: WRITE_IRREVERSIBLE, WRITE_RECOVERY_UNKNOWN
```

- MCP 도구는 이름만 보고 인자는 보지 않습니다. 결제 키·카드 번호·금액을 저장하거나 넘기지 않고, 결제 서버를 부르지 않습니다.
- 카드는 결제해도 된다는 뜻이 아닙니다. Claude Code 확인 창에서 당신이 정합니다.
- 엔진 확인(`check_action`)은 이번 첫 공개판에 들어 있지 않습니다. OLP 서버가 있어야 하는데, 아직 열리지 않았습니다.
- 상품 이름은 각 회사의 것이며, 제휴 관계를 뜻하지 않습니다.

## 한 번만 분류하기 (당신 터미널에서)

```
olp-hook classify --bash "cargo test" read_only
olp-hook classify --tool mcp__docs__search read_only
olp-hook classify --bash "make deploy" writes_other --reversible --recovery-declared
olp-hook classify --bash "psql" writes_other --db-count 42
```

- 답은 첫 낱말과 그 하위 명령(`cargo test`, `npm run`)까지만 덮습니다. 줄의 나머지에는 적용되지 않습니다.
- `--reversible`은 복구 위치도 함께 선언(`--recovery-declared`)해야 셉니다. 선언이 없으면 카드의 되돌리기 칸은 늘 확인 못 함으로 나옵니다.
- `--db-count`는 당신이 주는 숫자입니다. 카드에는 사용자가 준 숫자이고 훅이 확인하지 않았다고 나옵니다. 훅은 데이터베이스에 연결하지 않습니다.
- 지우거나 덮어쓰는 꼴(`rm`, `cp`, `mv` 등)과 `olp-hook` 자신은 분류할 수 없습니다.

## 세기 (기기 안에서만)

세기는 기본으로 켜져 있고, 함께 들어 있는 한도를 씁니다(`data/hook-config.default.json`: 세기 3000ms, 훅 전체 5000ms). 바꾸려면 `.olp/hook-config.json`에 적습니다. 적지 않은 칸은 기본값을 그대로 씁니다. 세기가 한도를 넘으면 카드에 세지 못했다고 나옵니다.

```json
{ "counter_timeout_ms": 3000, "hook_budget_ms": 5000 }
```

`counter_timeout_ms`는 `hook_budget_ms`보다 작아야 합니다. 위 Claude Code 설정의 `timeout`(초)은 `hook_budget_ms`보다 길어야 합니다. 훅이 예산을 넘으면 막습니다.

세는 쪽이 하는 일은 이것뿐입니다: 링크를 따라가지 않고 경로 목록 보기, `git status --porcelain`, `git rev-list --count <a>..<b>`. 원래 명령은 절대 실행하지 않습니다.

폴더 옮기기: `cd <폴더> && …`(Bash)는 그 폴더가 낱말 하나로 그대로 적혀 있고, 지금 있고, 같은 드라이브일 때만 그 폴더에서 셉니다. `;`·`||`·`|`·`&`·줄바꿈 뒤, 또는 변수·`~`·`-`·`pushd`·PowerShell `Set-Location` 이면 카드에 세지 못했다고 나옵니다. PowerShell 예외 1: `cd`/`Set-Location` 대상이 프로젝트 폴더 자체(전체 경로 또는 `.`)면 폴더가 바뀌지 않으므로 그대로 셉니다.

## 조용한 모드 (프로젝트마다, 기본 꺼짐)

`.olp/hook-config.json`에 `"quiet": true`를 넣습니다. 그러면 `writes_other --reversible --recovery-declared`로 분류한 쓰기는 카드 없이 지나갑니다. 지나간 것도 기록은 남습니다. 위험한 꼴과 돈 이동은 그래도 카드가 뜹니다.

## 내 숫자: `olp-hook stats`

이 기기 것만 셉니다: 처음 돈 시각, 고른 횟수, 멈춘 횟수(확인 창 또는 막음), 카드 뒤 실행된 호출과 안 된 호출, 조용히 지나간 횟수, 분류한 답. 경로·명령 글자·식별자는 없습니다. 아무것도 보내지 않습니다.

## 확인 창 옆에서 카드 보기: `olp-hook watch`

다른 터미널 탭에서 `olp-hook watch` 를 켜거나, `olp-hook watch --html --lang ko` 를 켜고 출력된 `file:///…/.olp/watch.html` 링크를 엽니다. 가장 최근 카드(하는 일·개수·되돌리기 상태)와 실행됐는지를 보여 주며, 이 기기의 `.olp/hook-log.jsonl` 만 읽습니다. 서버·포트·페이지 안 스크립트 0, 아무것도 보내지 않습니다. `--once` 는 한 번 그리고 끝납니다(페이지 자동 새로 고침 없음).

## 따로 켜는 하루 사용 숫자 (기본 꺼짐)

터미널에서 `npx @feedpia/olp install`을 마치면 한 번 묻습니다: "Send daily usage counts to FeedPia? (a random per-project ID that stays the same; never names, commands, or file names) [y/N]". Enter는 아니오입니다. 질문이 뜨기 전에 누른 키는 버리므로, 질문을 본 뒤 친 답만 셉니다. 이 질문에 사람이 `y`를 쳐야만 예입니다. 명령 옵션이나 환경 변수로는 예가 되지 않고, 프로젝트 안의 파일도 예로 치지 않습니다. 터미널 없이(파이프·CI·에이전트) 설치하면 묻지 않고 아무것도 보내지 않습니다. 손으로 설치한 프로젝트에서는 `olp-hook setup`이 같은 질문을 하고, 끈 뒤 다시 켤 때도 이것을 씁니다. 예라고 답한 사람만 보냅니다.

예라고 답하면 답과 무작위 식별자가 프로젝트 밖 `<홈>/olp/usage/`(프로젝트 폴더마다 한 파일, 이름은 폴더 경로의 해시)에 저장되고, `.claude/settings.json`에 `SessionStart` 칸 하나(`olp-hook signal --session-start`)가 더해집니다. 새 Claude Code 세션이 시작될 때 UTC 하루(00:00 UTC)에 많아야 한 번, 아래 칸만 보냅니다:

| 칸 | 뜻 |
|---|---|
| `v` | 형식 판(2) |
| `install_id` | 예라고 답할 때 만든 무작위 값 |
| `hook_version` | olp-hook 판 |
| `install_path` | `npx` 또는 `manual` |
| `used_today` | 지난번 보낸 뒤 훅이 돌았는지(예/아니오) |
| `age_days` | 이 기기에서 처음 돈 뒤 지난 날 수, 0–31(31은 31일 이상) |
| `cards_shown` | 허락 창에 보인 카드 수 |
| `blocked` | 훅이 막은 호출 수 |
| `decision_on` | 결정 카드(`decision` 설정)가 켜졌는지 |
| `decision_asked` | 보인 결정 카드 수 |
| `after_decision` | 결정 카드 뒤 10분 안 훅이 본 다음 지우기가 센 파일 모두(`all`)였는지, 더 적었는지(`fewer`), 없었는지(`none`). 훅이 다음에 본 것이지, 사람이 고른 답이 아닙니다(not the answer the person chose). |

숫자는 지난번 보낸 뒤부터 이번에 보내기 10분 전까지 훅이 남긴 기록입니다(결정 카드의 10분이 끝난 뒤에 셈). 숫자마다 10000에서 멈춥니다. 명령·경로·파일·폴더 이름·금액·답 글자는 보내지 않습니다. 서버는 IP 주소와 `install_id` 원문을 저장하지 않습니다. 서버 합계는 받은 날 기준입니다. 한 번 보낸 숫자에 지난번 보낸 뒤 여러 날이 들어 있을 수 있습니다.

동봉 설정 파일의 `https://` 주소로만 보냅니다. 넘겨주기(리다이렉트)는 따라가지 않고, 당신의 curl 설정 파일은 읽지 않으며, 5초에서 멈춥니다. 세션 시작 때는 훅이 기다리지 않아 세션이 느려지지 않고, `olp-hook signal` 을 직접 치면 기다렸다가 보냄 · 시간 안에 답 없음(도착했을 수 있음) · 보내지 않음 중 무엇인지 알려 줍니다. 프로젝트의 `.olp/hook-config.json`은 끄기만 할 수 있고(`"signal_endpoint": ""`), 주소를 바꿀 수는 없습니다. 실패해도 그날 다시 보내지 않고, 아무것도 출력하지 않으며, 훅의 판정은 바뀌지 않습니다. 훅 자체(PreToolUse/PostToolUse)는 보내지 않습니다.

언제든 `npx @feedpia/olp telemetry off`로 끕니다(`SessionStart` 칸과 식별자를 지움). `OLP_HOOK_SIGNAL=off`도 멈추고, 이것이 늘 이깁니다.

## 이 기기에 남는 것

- `.olp/hook-map.json`: 당신의 답. `tool:<이름>` 또는 `bash:<첫 낱말 + 하위 명령>` 마다 효과, 되돌릴 수 있는지, 복구 위치를 선언했는지, 시각, 당신이 준 숫자를 적습니다. 인자·경로·금액은 없습니다.
- `.olp/hook-log.jsonl`: 카드마다 보여 준 것(종류, 개수, 되돌리기 상태, 판정, 시각, 사람이 무엇을 골랐는지, 카드와 실행을 짝짓는 Claude Code 도구 호출 id). 경로·명령 글자·데이터베이스 이름은 없습니다.
- `.olp/first-run`, `.olp/used-day`: 시각 하나와 날짜 번호 하나.
- 프로젝트 밖 `<홈>/olp/usage/<해시>.json`: 사용 숫자 질문의 답(예면 무작위 식별자, `npx` 또는 `manual`, 마지막으로 보낸 때도).

Unix에서는 폴더가 `0700`, 파일이 `0600`입니다. Windows 에서는 `.olp/` 가 만들어진 폴더의 권한을 물려받고, 이 도구는 그 권한을 바꾸지 않습니다.

`olp-hook setup` 은 이 저장소의 `.git/info/exclude` 에 `.olp/` 를 한 번 더해, 이 파일들이 실수로 커밋되지 않게 합니다.

## 끄기

Claude Code 설정에서 `olp-hook` 줄을 지웁니다(`olp-hook uninstall`이 사용 숫자 `SessionStart` 칸까지 지웁니다). 사용 숫자만 멈추려면: `npx @feedpia/olp telemetry off`.

## 문제 해결

- **셸 명령이 모두 hook error 로 막힐 때:** Claude Code 가 `olp-hook` 을 실행하지 못했고(없음·고장·시간 초과), `"onFailure": "block"` 때문에 일부러 막은 것입니다. `.claude/settings.json` 의 `command` 경로에 `olp-hook` 파일이 실제로 있는지 확인하세요(기본은 홈 폴더의 `olp` 폴더, Windows 는 `olp-hook.exe`).
- 바로 끄려면 프로젝트의 `.claude/settings.json` 에서 이 안내로 넣은 `hooks` 칸을 빼세요. 그 프로젝트에서 바로 멈춥니다.

## 알려진 한계 (0.1.1)

- OLP가 아직 모르는 명령은 읽기만 해도 확인을 묻습니다. 아는 명령은 버전마다 늘려 갑니다.
- 느린 PC에서는 PowerShell 명령에 확인을 묻는 일이 잦아질 수 있습니다. 지우기·덮어쓰기 카드는 그대로 뜹니다.
- 되돌릴 수 있는 일은 묻지 않고 처리하는 «오퍼레이터»는 아직 없습니다. 만들고 있습니다.

## 한계

- 지울 폴더가 작업 폴더 자체이거나 그 위 폴더일 때(예: `rm -rf /경로/프로젝트`, `rm -rf .`) Claude Code는 자체 «Dangerous rm» 창을 띄우고 그 창에 OLP 카드를 넣지 않아, Yes 한 번이면 파일 목록 없이 지워집니다. 이 꼴은 OLP가 묻지 않고 막습니다. 아무것도 지워지지 않고, 카드는 에이전트에게 가며, 상대 경로로 다시 하거나 터미널에서 직접 하라는 안내가 붙습니다. PowerShell `Remove-Item`은 해당하지 않습니다. OLP가 어느 폴더인지 풀 수 없는 꼴(변수 `"$DIR"/*`, `bash -c "rm …"`, `/*` 무늬)은 지금처럼 묻기만 하므로, 그 꼴에서는 Claude Code 자체 창이 먼저 뜰 수 있습니다.
- `PreToolUse` 칸의 `"onFailure": "block"` 은 `olp-hook` 이 없거나, 실패하거나, `timeout` 을 넘기면 Claude Code 가 그 호출을 막게 합니다(Claude Code v2.1.295 이상). 이 칸이 없거나 Claude Code 가 그보다 옛 판이면 도구가 그냥 실행될 수 있습니다. `timeout` 은 `hook_budget_ms` 보다 길게 두세요. 훅은 예산(기본 5초)에서 스스로 멈추므로, 10초 `timeout` 은 Claude Code 가 끼어들기 전에 여유를 줍니다.
- `read_only`로 표시한 명령 묶음(예: `python`)은 통째로 지나갑니다. 정말 읽기만 하는 묶음만 표시하세요. `git` 자체를 `read_only`로 표시하지 마세요.
- 이번 판은 Edit/Write를 걸지 않아서, 에이전트가 파일 편집 도구로 `.olp/hook-map.json`이나 `.claude/settings.json`을 바꿀 수 있습니다. 위험한 여섯 꼴은 그래도 잡히지만, 다른 답은 바뀔 수 있습니다. 다음 판은 답을 프로젝트 밖(홈 폴더)에 둡니다.
- 같은 까닭으로, 에이전트가 파일 편집 도구로 `<홈>/olp/usage/`에 사용 숫자 답을 써서 보내기를 켤 수 있습니다. 그래도 나가는 것은 위 표의 숫자뿐입니다.
- 카드 뒤 실행 안 됨은 호출이 실행됐다고 알려 오지 않았다는 뜻입니다. 확인 창에서 거절(No)한 것과 실행이 실패한 것을 구별하지 못합니다.
- 숫자는 읽기만 해서 미리 본 값입니다. 정확히 무엇이 사라질지에 대한 약속이 아닙니다.
- PowerShell(Windows 에서 Claude Code 가 쓰는 셸)에서는 `Remove-Item` 과 그 짧은 이름(`del`·`rd`·`ri`·`rm`·`rmdir`·`erase`)을 경로가 글자로 적혀 있을 때 셉니다. 변수·와일드카드·파이프로 받은 입력·`-Filter`/`-Include`/`-Exclude`/`-WhatIf`, `HKCU:`·`Env:` 같은 파일 아닌 경로는 "count not checked" 로 나옵니다. `Get-ChildItem`·`Test-Path`·`Measure-Object` 같은 읽기 명령은 카드 없이 지나갑니다.
- PowerShell 에서 따옴표 글자만 있는 줄(`"--- build ---"` 같은 것)은 글자를 찍기만 하므로 카드 없이 지나갑니다. 안에 `$(…)` 가 든 글자, `&`·`.` 로 실행하는 글자는 예외입니다. `2>$null`·`>$null` 은 출력을 버리는 것이라 파일 쓰기로 세지 않습니다.
- Windows 에서 `{ }` 블록(`Where-Object`·`ForEach-Object` 등)이 든 PowerShell 명령은 PowerShell 자체 구문 해석기로 읽습니다. 해석기는 글자만 읽고 실행하지 않습니다. 블록 안의 명령·속성 읽기·글자 메서드를 확인하고, `.Delete()` 처럼 파일을 바꾸는 메서드에는 카드를 띄웁니다. 알아보지 못한 것은 쓰기로 보고 "Not recognized:" 줄을 붙입니다. 해석기를 쓸 수 없는 곳(macOS·Linux, 또는 1.5초를 넘길 때)에서는 이런 명령을 쓰기로 봅니다. 해석기를 끄려면 Claude Code 에 환경 변수 `OLP_HOOK_PS_PARSER=off` 를 주세요. 그러면 훅이 이런 명령을 쓰기로 봅니다.

라이선스: MIT. (영어판 [README.md](README.md)가 정본이고, 이 한국어판은 같은 내용을 한국어로 따로 쓴 것입니다.)
