#!/usr/bin/env bash
# Unit tests for plugin scripts: tests/plugins/test-*.lua under LuaJIT (CI) with the
# mpv API stub in tests/plugins/stub.lua. Run from anywhere.
set -euo pipefail
cd "$(dirname "$0")/../.."
LUA=${LUA:-luajit}
rc=0
XDG_DATA_HOME=$(mktemp -d)
export XDG_DATA_HOME VIVI_TEST_NO_SYSTEM_TOOLS=1
trap 'rm -rf "$XDG_DATA_HOME"' EXIT
for t in tests/plugins/test-*.lua; do "$LUA" "$t" || rc=1; done
exit $rc
