#!/usr/bin/env bash
# Pastes three lines into the TUI running in tmux and captures the screen.
# The only registered model points at a loopback port that never answers, and
# the environment is cleared, so no provider key is present and nothing leaves
# the machine.
#
#   tui_paste_check.sh <scratch dir> <repository>
#
# The scratch directory must hold stall_server.py and tui-models.toml from
# this directory, an empty Git repository named tui-ws2 used as the
# workspace, and target/debug/socium-tui built from the reviewed commit.
# The screen is written to <scratch dir>/tui-paste.txt.
set -uo pipefail
S=$1
REPO=$2
SOCK=sociumreview

python3 -I "$S/stall_server.py" &
server=$!
trap 'tmux -L $SOCK kill-server 2>/dev/null; kill $server 2>/dev/null' EXIT

cat > "$S/tui-launch.sh" <<EOF
#!/usr/bin/env bash
cd "$S/tui-ws2"
exec env -i PATH="\$PATH" HOME="\$HOME" TERM=xterm-256color LANG=C.UTF-8 \\
  LOCAL_KEY=dummy SOCIUM_MODELS_FILE="$S/tui-models.toml" \\
  "$S/target/debug/socium-tui"
EOF
chmod +x "$S/tui-launch.sh"

tmux -L $SOCK new-session -d -x 110 -y 34 -s r "nix develop $REPO -c $S/tui-launch.sh; sleep 30"
for _ in $(seq 1 120); do
  tmux -L $SOCK capture-pane -p -t r | grep -q "local" && break
  sleep 0.5
done
sleep 1
{
  echo "=== screen after start ==="
  tmux -L $SOCK capture-pane -p -t r
} > "$S/tui-paste.txt"

printf 'first pasted line\nsecond pasted line\nthird pasted line' > "$S/paste-buffer.txt"
tmux -L $SOCK load-buffer "$S/paste-buffer.txt"
tmux -L $SOCK paste-buffer -p -t r
sleep 3
{
  echo
  echo "=== screen 3 seconds after pasting three lines (tmux paste-buffer -p) ==="
  tmux -L $SOCK capture-pane -p -t r
} >> "$S/tui-paste.txt"

# Cancel the stalled request, then show what is left in the composer.
tmux -L $SOCK send-keys -t r Escape
sleep 3
{
  echo
  echo "=== screen after Esc (cancel) ==="
  tmux -L $SOCK capture-pane -p -t r
} >> "$S/tui-paste.txt"
tmux -L $SOCK send-keys -t r C-c
sleep 1
