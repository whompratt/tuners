#!/bin/bash
# Windows-side dev loop (plan 010 phase 5): sync the repo to the Windows
# filesystem and run `pnpm tauri dev` there, so the app sees real FH6 UDP
# telemetry (the game can't reach a WSL socket). Needs Node + pnpm installed
# on the Windows side; the first run installs app/node_modules there.
#
# Usage: ./run.sh [KEY=VAL ...]   (leading KEY=VAL args become Windows env vars)

set -e

dir=${PWD##*/}
tmp_path="/mnt/c/temp/$dir"
src_path=$PWD

mkdir -p "$tmp_path"

# Windows gets its own target/ and node_modules (host-specific artifacts) —
# rsync must neither copy nor delete them.
rsync -rq . "$tmp_path" --exclude-from=.gitignore --delete \
    --exclude node_modules --exclude /app/build --exclude /app/.svelte-kit

cd "$tmp_path"

powershell.exe -Command "Get-Process tuners-app -ErrorAction SilentlyContinue | Stop-Process -Force" 2>/dev/null

# Data root stays in the WSL source tree so recordings/journals survive the
# next rsync --delete. WSLENV tells WSL which env vars to forward to the
# Windows side; the `/p` suffix translates the path to Windows form.
export TUNERS_DATA="$src_path"
export WSLENV="${WSLENV:+$WSLENV:}TUNERS_DATA/p"

# Strip leading KEY=VAL args and inject them as PowerShell env vars.
env_prefix=""
while [[ "$1" == *=* && "$1" != -* ]]; do
    key="${1%%=*}"
    val="${1#*=}"
    env_prefix+="\$env:${key}='${val}'; "
    shift
done

if ! powershell.exe -Command "Get-Command pnpm -ErrorAction Stop | Out-Null" 2>/dev/null; then
    echo "pnpm not found on the Windows side — install Node, then 'npm install -g pnpm'" >&2
    exit 1
fi

cd app
if [ ! -d node_modules ]; then
    echo "first run: installing app/node_modules on the Windows side…"
    powershell.exe -Command "pnpm install"
fi

powershell.exe -Command "${env_prefix}pnpm tauri dev $*"
