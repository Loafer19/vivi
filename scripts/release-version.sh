#!/usr/bin/env bash
# Prints the asset / installer version name:
#   tag builds (GITHUB_REF_NAME=vX.Y.Z) → X.Y.Z
#   anything else → <Cargo.toml version>-dev.<shortsha>
# Usage: scripts/release-version.sh [Cargo.toml]
set -euo pipefail
if [[ "${GITHUB_REF_NAME:-}" == v[0-9]* ]]; then
  echo "${GITHUB_REF_NAME#v}"
  exit 0
fi
toml=${1:-Cargo.toml}
ver=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$toml" | head -1)
if [[ -z $ver ]]; then
  echo "release-version: no version in $toml" >&2
  exit 1
fi
sha=${GITHUB_SHA:-}
if [[ -z $sha ]]; then
  sha=$(git -C "$(dirname "$toml")" rev-parse HEAD)
fi
echo "${ver}-dev.${sha:0:7}"
