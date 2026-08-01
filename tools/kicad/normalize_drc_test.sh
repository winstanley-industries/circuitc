#!/bin/bash
set -euo pipefail

normalizer="$1"
first_raw="$2"
second_raw="$3"
unexpected_raw="$4"
first_normalized="${TEST_TMPDIR}/first.normalized.json"
second_normalized="${TEST_TMPDIR}/second.normalized.json"

python3 "${normalizer}" \
  --raw "${first_raw}" \
  --normalized "${first_normalized}" \
  --expected-major 10 \
  --allow-library-warning R1 \
  --allow-library-warning R2
python3 "${normalizer}" \
  --raw "${second_raw}" \
  --normalized "${second_normalized}" \
  --expected-major 10 \
  --allow-library-warning R1 \
  --allow-library-warning R2
cmp "${first_normalized}" "${second_normalized}"

if python3 "${normalizer}" \
  --raw "${unexpected_raw}" \
  --normalized "${TEST_TMPDIR}/unexpected.normalized.json" \
  --expected-major 10 \
  --allow-library-warning R1 \
  --allow-library-warning R2; then
  echo "normalizer accepted an unexpected KiCad error" >&2
  exit 1
fi
