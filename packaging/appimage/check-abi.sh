#!/usr/bin/env bash
# Fail if any ELF in an AppDir needs a glibc or libstdc++ newer than the oldest
# system the AppImage supports (Ubuntu 22.04: glibc 2.35, GCC 12 libstdc++).
# Weak references (e.g. optional pidfd_* calls) are allowed.
# Usage: check-abi.sh <AppDir> [max-glibc] [max-glibcxx]
set -euo pipefail
DIR=${1:?usage: check-abi.sh <AppDir>}
MAX_GLIBC=${2:-2.35}
MAX_GLIBCXX=${3:-3.4.30}
newer() { [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n1)" = "$1" ]; }
fail=0; n=0
while IFS= read -r -d '' f; do
  head -c 4 "$f" 2>/dev/null | grep -q $'\x7fELF' || continue
  n=$((n + 1))
  # objdump -T: undefined, non-weak symbols with a version, e.g. "... DF *UND* ... (GLIBC_2.38) foo"
  while read -r tag ver; do
    case $tag in
      GLIBC) newer "$ver" "$MAX_GLIBC" && { echo "$f needs GLIBC_$ver (max $MAX_GLIBC)"; fail=1; } ;;
      GLIBCXX) newer "$ver" "$MAX_GLIBCXX" && { echo "$f needs GLIBCXX_$ver (max $MAX_GLIBCXX)"; fail=1; } ;;
    esac
  done < <(objdump -T "$f" 2>/dev/null | awk '/\*UND\*/ && $2 != "w" { if (match($0, /\((GLIBC|GLIBCXX)_[0-9.]+\)/)) { v = substr($0, RSTART + 1, RLENGTH - 2); sub(/_/, " ", v); print v } }' | sort -u)
done < <(find "$DIR" -type f -print0)
echo "checked $n ELF files against glibc $MAX_GLIBC / GLIBCXX $MAX_GLIBCXX"
exit $fail
