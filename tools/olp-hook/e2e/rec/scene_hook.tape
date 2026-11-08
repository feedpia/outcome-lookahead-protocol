# Scene 1: npx install in a clean folder → registered hook → a read passes, a delete gets a card.
# Without Claude Code: feed the registered command the same input Claude Code sends right before using a tool (hookdemo.js).
# @slot@ values are filled by rec_prep.sh. All screen text is real execution.
Output "@OUT@/olp-hook-@OS@.mp4"
Output "@OUT@/olp-hook-@OS@.gif"
Set Shell @SHELL@
Set FontSize 18
Set Width 1280
Set Height 760
Set TypingSpeed 35ms
Set Theme "Dracula"
Env HOME "@HOME@"
Env USERPROFILE "@HOME@"
Env PS1 "$ "

Hide
Type "cd '@PROJ@'"
Enter
Type "clear"
Enter
Sleep 500ms
Show

Type "# OLP hook demo (@OS@) - no Claude Code here: we send the hook the same input Claude Code sends"
Enter
Sleep 1s
Type "npx olp install"
Enter
Wait+Screen@30s /\[y\/N\]/
Sleep 800ms
Type "n"
Enter
Wait+Screen@15s /Nothing will be sent/
Sleep 1500ms

Type "node show-settings.js"
Enter
Sleep 2500ms

Type "clear"
Enter
Type `node hookdemo.js "git status --short"`
Enter
Wait+Screen@20s /OLP: pass/
Sleep 2500ms

Type `node hookdemo.js "rm -rf build"`
Enter
Wait+Screen@20s /OLP: ask/
Sleep 5s

Type "ls build"
Enter
Sleep 2500ms
