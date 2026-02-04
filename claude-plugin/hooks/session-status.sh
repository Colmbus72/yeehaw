#!/bin/bash
# Yeehaw Session Status Hook
# Writes session status to signal file for the Yeehaw CLI to read

STATUS="$1"
PANE_ID="${TMUX_PANE:-unknown}"
SIGNAL_DIR="$HOME/.yeehaw/session-signals"
SIGNAL_FILE="$SIGNAL_DIR/${PANE_ID//[^a-zA-Z0-9]/_}.json"

mkdir -p "$SIGNAL_DIR"
cat > "$SIGNAL_FILE" << EOF
{"status":"$STATUS","updated":$(date +%s)}
EOF
