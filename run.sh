#!/usr/bin/env bash
# Build and run Findex.
# usage: ./run.sh [--release] [path-or-findex-args ...]   (default path: $HOME)
set -euo pipefail
cd "$(dirname "$0")"
profile=()
[ "${1:-}" = "--release" ] && { profile=(--release); shift; }
exec cargo run "${profile[@]}" -p findex-app -- "${@:-$HOME}"
