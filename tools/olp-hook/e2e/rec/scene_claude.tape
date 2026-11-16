# Scene 2: real interactive Claude Code screen — both a test and a video.
# Assertion = Wait+Screen: if the text does not appear in time, vhs ends with a failure (= red test).
# ① ask to delete → within 120 s a permission prompt (proceed?) + OLP card (the prompt shows the card reason «[OLP] Before this runs: delete files …») → Escape (no) → files remain in ls build
#  ② ask to look around → result within 90 s (?? build/) — Claude Code's own prompt is suppressed by user settings; if it appears, no result shows and the test is red
# Only the request text is @LANG@ (Korean/English); card text stays English. The start screen (may contain an account line) is hidden.
# @slot@ values are filled by rec_prep.sh.
Output "@OUT@/olp-claude-@OS@-@LANG@.mp4"
Output "@OUT@/olp-claude-@OS@-@LANG@.gif"
Output "@OUT@/olp-claude-@OS@-@LANG@.txt"
Set Shell bash
Set FontSize 18
Set Width 1280
Set Height 760
Set TypingSpeed 35ms
Set Theme "Dracula"
Env HOME "@HOME@"
Env PS1 "$ "
# Find claude even if the macOS login shell rebuilds PATH — no path on screen (recorded text privacy check). The original PATH is appended so vhs's ttyd and ffmpeg still work
Env PATH "@CLAUDEDIR@:@PATH@"

Hide
Type "cd '@PROJ@' && clear"
Enter
Sleep 500ms
Show
Type "# OLP 0.1.3 · @OS@ · Claude Code @CCVER@ · @LANG@  (첫 실행 안내는 건너뜀)"
Enter
Sleep 2500ms

Hide
Type "claude --model haiku"
Enter
Wait+Screen@120s /for shortcuts|\? for|Try "|auto mode on|shift\+tab to cycle/
Sleep 1s
Type "/clear"
Enter
Sleep 2s
Show

Type "@DEL@"
Sleep 500ms
Enter
Wait+Screen@120s /proceed\?/
Wait+Screen@10s /\[OLP\] Before this runs: delete files/
Sleep 6s
Escape
Sleep 3s

Type "@READ@"
Sleep 500ms
Enter
Wait+Screen@90s /\?\? build\//
Sleep 4s

Ctrl+C
Sleep 500ms
Ctrl+C
Sleep 1s
Type "ls build"
Enter
Wait+Screen@10s /a\.log/
Sleep 2500ms
