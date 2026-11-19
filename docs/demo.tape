# The README recording of the picker, in demo mode (fixture machines, no network).
# Regenerate with scripts/demo.sh; the binary is mounted at /opt/gliffherdr.
Output docs/assets/picker.gif

Set Shell bash
Set FontSize 16
Set Width 1200
Set Height 560
Set Theme "TokyoNight"
Set Padding 16
Set TypingSpeed 60ms
Set PlaybackSpeed 1

Env GLIFFHERDR_DEMO "1"

Hide
Type "export PATH=/opt/gliffherdr:$PATH PS1='$ ' && clear"
Enter
Type "gliffherdr pick"
Enter
Show

# Let the checks settle: kiosk, render and nas answer after 0.8, 1.2 and 2 seconds.
Sleep 2.5s
Screenshot docs/assets/picker.png

# lab needs setup: the details panel explains it and offers the fix key.
Down 5
Sleep 2s

# Filtering: matched characters are underlined.
Type "re"
Sleep 1.5s
Backspace 2
Sleep 600ms

# The screen chooser for j.
PageUp
Down
Sleep 400ms
Ctrl+O
Sleep 2s
Down
Sleep 600ms
Escape
Sleep 600ms

# Connecting to a machine that is not ready explains why, and how to fix it.
Down 3
Sleep 500ms
Enter
Sleep 2500ms
Escape
Sleep 500ms

# studio already has a window: Enter focuses it and the picker closes.
PageUp
Sleep 800ms
Enter
Sleep 1s
