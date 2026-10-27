#!/usr/bin/env bash
# Forbids the upstream project's site, Discord invite and vendor name in
# release-facing files (the exact strings are the PATTERNS list below). Run by
# .github/workflows/packaging-lint.yml (and by hand, from anywhere). Provenance
# mentions elsewhere (README, NOTICE, licences, ATTRIBUTION) are intentional and
# are not scanned.
#
#   packaging/lint-brand.sh

# shellcheck shell=bash
set -euo pipefail
cd "$(dirname "$0")/.."

# Everything the release pipeline ships or builds from: packaging metadata, CI
# workflows and the web build's page (its <title> is the browser tab label).
DIRS=(packaging .github/workflows apps/pdfcraft-web)

PATTERNS=(
  'getartcraft\.com'
  'discord\.gg/artcraft'
  'Learning Machines LLC'
)

found=0
for pattern in "${PATTERNS[@]}"; do
  out=""
  status=0
  out="$(grep -RniE "$pattern" --exclude='lint-brand.sh' "${DIRS[@]}")" || status=$?
  if [[ "$status" -eq 0 ]]; then
    printf '%s\n' "$out"
    echo "error: forbidden upstream reference matching /$pattern/ above" >&2
    found=1
  elif [[ "$status" -ne 1 ]]; then
    echo "error: search for /$pattern/ failed (grep status $status)" >&2
    exit 1
  fi
done

if [[ "$found" -ne 0 ]]; then
  echo "error: release metadata must point at github.com/wugaxp/pdf-forge, not the upstream site" >&2
  exit 1
fi
echo "ok: no upstream links in packaging, workflows or the web page"
