#!/usr/bin/env bash
# Runs every scripted UI suite against a fresh fixture. Needs a display (X11/Wayland) or a virtual one.
# usage: tests/ui/run.sh [suite ...]   (default: all)   FINDEX=path/to/findex to test another build
set -u
root="$(cd "$(dirname "$0")/../.." && pwd)"
bin="${FINDEX:-$root/target/debug/findex}"
out="${FINDEX_UI_TEST_OUT:-$root/target/ui-test}"
fixture="${FINDEX_FIXTURE:-$HOME/.cache/findex-ui-fixture}"
suites=("$@")
[ ${#suites[@]} -eq 0 ] && suites=(views navigation operations shell settings features)
failed=0
for s in "${suites[@]}"; do
  rm -rf "$fixture"
  python3 "$root/development/scripts/make_fixture.py" "$fixture"
  # Isolated config so the user's settings are neither read nor overwritten.
  cfg="$(mktemp -d)"
  if XDG_CONFIG_HOME="$cfg" FINDEX_UI_TEST="$root/tests/ui/$s.txt" FINDEX_UI_TEST_OUT="$out/$s" \
     timeout 600 "$bin" "$fixture" > "$out-$s.log" 2>&1; then
    echo "PASS $s ($(tail -1 "$out-$s.log"))"
  else
    echo "FAIL $s"; grep FAIL "$out-$s.log"; failed=1
  fi
  rm -rf "$cfg"
done
rm -rf "$fixture"
exit $failed
