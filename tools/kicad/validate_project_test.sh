#!/bin/bash
set -euo pipefail

validator="$1"
valid_project="$2"
invalid_json_project="$3"
invalid_structure_project="$4"
invalid_content_project="$5"

mkdir -p \
  "${TEST_TMPDIR}/valid" \
  "${TEST_TMPDIR}/invalid-json" \
  "${TEST_TMPDIR}/invalid-structure" \
  "${TEST_TMPDIR}/invalid-content"
cp "${valid_project}" "${TEST_TMPDIR}/valid/voltage_divider.kicad_pro"
cp "${invalid_json_project}" "${TEST_TMPDIR}/invalid-json/voltage_divider.kicad_pro"
cp "${invalid_structure_project}" \
  "${TEST_TMPDIR}/invalid-structure/voltage_divider.kicad_pro"
cp "${invalid_content_project}" \
  "${TEST_TMPDIR}/invalid-content/voltage_divider.kicad_pro"

python3 "${validator}" \
  --project "${TEST_TMPDIR}/valid/voltage_divider.kicad_pro" \
  --expected-filename voltage_divider.kicad_pro \
  --normalized "${TEST_TMPDIR}/project.normalized.json"
grep -F '"artifact_kind": "kicad_project"' "${TEST_TMPDIR}/project.normalized.json"

if python3 "${validator}" \
  --project "${TEST_TMPDIR}/invalid-json/voltage_divider.kicad_pro" \
  --expected-filename voltage_divider.kicad_pro \
  --normalized "${TEST_TMPDIR}/invalid-json.normalized.json"; then
  echo "project validator accepted invalid JSON" >&2
  exit 1
fi

if python3 "${validator}" \
  --project "${TEST_TMPDIR}/invalid-structure/voltage_divider.kicad_pro" \
  --expected-filename voltage_divider.kicad_pro \
  --normalized "${TEST_TMPDIR}/invalid-structure.normalized.json"; then
  echo "project validator accepted invalid project structure" >&2
  exit 1
fi

if python3 "${validator}" \
  --project "${TEST_TMPDIR}/invalid-content/voltage_divider.kicad_pro" \
  --expected-filename voltage_divider.kicad_pro \
  --normalized "${TEST_TMPDIR}/invalid-content.normalized.json"; then
  echo "project validator accepted unexpected nested content" >&2
  exit 1
fi

if python3 "${validator}" \
  --project "${TEST_TMPDIR}/valid/voltage_divider.kicad_pro" \
  --expected-filename wrong.kicad_pro \
  --normalized "${TEST_TMPDIR}/wrong-filename.normalized.json"; then
  echo "project validator accepted a mismatched artifact filename" >&2
  exit 1
fi

cp "${valid_project}" "${TEST_TMPDIR}/renamed.kicad_pro"
if python3 "${validator}" \
  --project "${TEST_TMPDIR}/renamed.kicad_pro" \
  --expected-filename voltage_divider.kicad_pro \
  --normalized "${TEST_TMPDIR}/renamed.normalized.json"; then
  echo "project validator accepted a renamed artifact" >&2
  exit 1
fi

if python3 "${validator}" \
  --project "${TEST_TMPDIR}/valid/voltage_divider.kicad_pro" \
  --expected-filename ../voltage_divider.kicad_pro \
  --normalized "${TEST_TMPDIR}/noncanonical.normalized.json"; then
  echo "project validator accepted a noncanonical expected filename" >&2
  exit 1
fi
