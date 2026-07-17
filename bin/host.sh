#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

FEATURES=client
ADDR=127.0.0.1:33998

launch_client() {
    local name="$1"
    local inner="cd $(printf '%q' "$PWD"); sleep 1; cargo run --features $FEATURES -- join $ADDR $name; echo; echo Exited; read -rn1"
    if command -v gnome-terminal >/dev/null 2>&1; then
        gnome-terminal --title="ssnt: $name" -- bash -c "$inner"
    elif command -v konsole >/dev/null 2>&1; then
        konsole -p "tabtitle=ssnt: $name" -e bash -c "$inner"
    elif command -v xterm >/dev/null 2>&1; then
        xterm -T "ssnt: $name" -e bash -c "$inner"
    elif command -v x-terminal-emulator >/dev/null 2>&1; then
        x-terminal-emulator -e bash -c "$inner"
    else
        echo "No terminal emulator found; running client '$name' in the background." >&2
        bash -c "$inner" &
    fi
}

cargo build --features "$FEATURES" || { echo "Failed to build" >&2; exit 1; }

launch_client Kerfus &

cargo run --features "$FEATURES" -- host "$ADDR"
