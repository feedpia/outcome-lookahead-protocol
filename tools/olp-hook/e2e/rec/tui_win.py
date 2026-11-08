# Windows job: real interactive Claude Code through pywinpty (ConPTY).
# Start the real interactive Claude Code screen with pywinpty (Windows ConPTY), assert on the screen text, and save the received text as an asciinema cast (v2).
# Usage (CI runners only): python tui_win.py <proj> <out_dir> <lang en|ko> <claude version>
# Assertions: delete = «Do you want to proceed» + «OLP hook» within 120 s → Esc (no) → build files remain
#               read = «?? build/» within 90 s (the fake home allows git status, so Claude Code's own prompt does not appear)
# On a wrong result retry once; if still wrong, fail and save the screen text. Nothing staged — the cast is the received text as is.
import json, os, re, shutil, sys, time
from winpty import PtyProcess

proj, out, lang, ccver = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
PROMPTS = {
    'en': ('Delete the build folder with the Bash tool: rm -rf build',
           'Run git status --short with the Bash tool and show me the output'),
    'ko': ('Bash 도구로 build 폴더를 지워 줘: rm -rf build',
           'Bash 도구로 git status --short 를 실행하고 결과를 보여 줘'),
}
ANSI = re.compile(r'\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*(\x07|\x1b\\)|\x1b[()][A-Z0-9]|\x1b[=>]')
W, H = 130, 40
key = 'win32-x64'
T0 = time.time()


def log(m):
    print(f'[{time.time()-T0:6.1f}s] {m}', flush=True)
base = os.path.join(out, f'olp-claude-{key}-{lang}')


def flat(s):
    return ANSI.sub('', s).replace(' ', '').replace('\r', '').replace('\n', '')


class Run:
    """읽기는 뒤 실(thread)에서만 — p.read 는 화면이 멈추면 끝없이 기다리므로(11-07 실행 42355938531 30분 막힘) 앞쪽은 큐만 봄."""
    def __init__(self):
        import queue, threading
        env = dict(os.environ)
        claude = shutil.which('claude') or 'claude'
        log(f'spawn {claude}')
        self.p = PtyProcess.spawn('claude --model haiku', cwd=proj, env=env, dimensions=(H, W)) # claude from PATH
        self.buf = ''          # everything (for assertions)
        self.events = []       # cast (recorded part only)
        self.rec_t0 = None
        self.q = queue.Queue()
        def reader():
            while True:
                try:
                    c = self.p.read(65536)
                except Exception:
                    self.q.put(None); return
                if c:
                    self.q.put(c)
        threading.Thread(target=reader, daemon=True).start()

    def pump(self, secs):
        import queue
        end = time.time() + secs
        while True:
            left = end - time.time()
            try:
                c = self.q.get(timeout=max(0.05, min(0.2, left))) if left > 0 else self.q.get_nowait()
            except queue.Empty:
                if time.time() >= end:
                    return
                continue
            if c is None:
                return
            self.buf += c
            if self.rec_t0 is not None:
                self.events.append([round(time.time() - self.rec_t0, 3), 'o', c])

    def wait_any(self, groups, secs, since=0, nudge=0):
        # every nudge seconds send Ctrl+L (redraw): on a second run, Windows ConPTY sends no screen until a key is pressed
        end = time.time() + secs
        last = time.time()
        while time.time() < end:
            if nudge and time.time() - last > nudge:
                try:
                    self.p.write('')
                except Exception:
                    pass
                last = time.time()
            self.pump(0.5)
            f = flat(self.buf[since:])
            if any(all(n in f for n in g) for g in groups):
                return True
        return False

    def wait_for(self, needles, secs, since=0):
        end = time.time() + secs
        while time.time() < end:
            self.pump(0.5)
            f = flat(self.buf[since:])
            if all(n in f for n in needles):
                return True
        return False

    def send(self, s, gap=0.05):
        for ch in s:
            self.p.write(ch)
            time.sleep(gap)
            self.pump(0.0)

    def close(self):
        for k in ['\x03', '\x03']:
            try:
                self.p.write(k); self.pump(1)
            except Exception:
                pass
        try:
            self.p.terminate(force=True)
        except Exception:
            pass


def attempt():
    r = Run()
    try:
        # start screen (may contain an account line) = not recorded
        # input-ready markers: older versions «? for shortcuts», 2.1.319 default auto mode «auto mode on (shift+tab to cycle)»
        if not r.wait_any([['forshortcuts'], ['automodeon'], ['shift+tabtocycle']], 120, nudge=8):
            return False, r, 'claude 입력 칸이 120초 안에 안 뜸'
        log('claude 입력 칸 뜸 → /clear'); r.send('/clear'); r.pump(0.5); r.p.write('\r'); r.pump(2)
        # from here: recording + captions
        r.rec_t0 = time.time()
        r.events.append([0.0, 'o', f'\x1b[2J\x1b[H# OLP 0.1.0 · {key} · Claude Code {ccver} · {lang}  (첫 실행 안내는 건너뜀)\r\n'])
        r.p.write('\x0c'); r.pump(1)   # redraw the screen
        mark = len(r.buf)
        log('지우기 부탁 보냄'); r.send(PROMPTS[lang][0]); r.pump(0.5); r.p.write('\r')
        if not r.wait_for(['Doyouwanttoproceed', '[OLP]Beforethisruns:deletefiles'], 120, since=mark):
            return False, r, '지우기: 120초 안에 허락 창 + OLP 카드([OLP] Before this runs: delete files) 가 같이 안 보임'
        log("허락 창 + 카드 봄 → Esc"); r.pump(6)
        r.p.write('\x1b'); r.pump(3)   # human answer = no
        mark = len(r.buf)
        log('둘러보기 부탁 보냄'); r.send(PROMPTS[lang][1]); r.pump(0.5); r.p.write('\r')
        if not r.wait_for(['??build/'], 90, since=mark):
            return False, r, '둘러보기: 90초 안에 결과(?? build/)가 안 보임(창이 떴거나 엉뚱한 일)'
        r.pump(4)
        if not os.path.isfile(os.path.join(proj, 'build', 'a.log')):
            return False, r, 'build/a.log 가 없어짐(아니오인데 지워짐)'
        return True, r, 'PASS'
    finally:
        r.close()


def save(r, suffix=''):
    cast = base + suffix + '.cast'
    with open(cast, 'w', encoding='utf-8') as f:
        f.write(json.dumps({'version': 2, 'width': W, 'height': H, 'env': {'TERM': 'xterm-256color'}}) + '\n')
        for e in r.events:
            f.write(json.dumps(e, ensure_ascii=False) + '\n')
    with open(base + suffix + '.txt', 'w', encoding='utf-8') as f:
        f.write(ANSI.sub('', ''.join(e[2] for e in r.events)))
    return cast


for t in (1, 2):
    ok, r, why = attempt()
    print(f'{key} {lang} 시도 {t}: {why}')
    if ok:
        save(r)
        print(f'PASS {key} {lang}: 지우기 = 허락 창 + OLP 카드 → 아니오 → 파일 남음 / 둘러보기 = 창 없이 결과')
        sys.exit(0)
    save(r, f'-fail-try{t}')
    with open(os.path.join(out, f'fail-{key}-{lang}-try{t}.txt'), 'w', encoding='utf-8') as f:
        f.write(why + '\n\n' + ANSI.sub('', r.buf)[-6000:])
print(f'FAIL {key} {lang} (2번 다)')
sys.exit(1)
