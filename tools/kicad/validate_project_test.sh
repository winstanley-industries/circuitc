#!/bin/bash
set -euo pipefail

validator="$1"
valid_project="$2"
invalid_json_project="$3"
invalid_structure_project="$4"
invalid_content_project="$5"
invalid_meta_filename_project="$6"
invalid_meta_shape_project="$7"
invalid_version_project="$8"
invalid_version_type_project="$9"
invalid_libraries_shape_project="${10}"
invalid_libraries_content_project="${11}"
invalid_list_content_project="${12}"

expect_fixture_failure() {
  local label="$1"
  local expected="$2"
  local fixture="$3"
  local directory="${TEST_TMPDIR}/${label}"
  mkdir -p "${directory}"
  cp "${fixture}" "${directory}/voltage_divider.kicad_pro"
  if python3 "${validator}" \
    --project "${directory}/voltage_divider.kicad_pro" \
    --expected-filename voltage_divider.kicad_pro \
    --normalized "${directory}/normalized.json" \
    >"${directory}/stdout" 2>"${directory}/stderr"; then
    echo "project validator accepted ${label}" >&2
    exit 1
  fi
  grep -F "${expected}" "${directory}/stderr"
}

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

expect_fixture_failure invalid-meta-filename \
  'KiCad project filename does not match its artifact' \
  "${invalid_meta_filename_project}"
expect_fixture_failure invalid-meta-shape \
  'KiCad project meta must contain filename and version' \
  "${invalid_meta_shape_project}"
expect_fixture_failure invalid-version \
  'KiCad project meta version must be integer 1' \
  "${invalid_version_project}"
expect_fixture_failure invalid-version-type \
  'KiCad project meta version must be integer 1' \
  "${invalid_version_type_project}"
expect_fixture_failure invalid-libraries-shape \
  'KiCad project libraries must contain pinned footprint and symbol lists' \
  "${invalid_libraries_shape_project}"
expect_fixture_failure invalid-libraries-content \
  "KiCad project library field 'pinned_symbol_libs' must be an empty list" \
  "${invalid_libraries_content_project}"
expect_fixture_failure invalid-list-content \
  "KiCad project field 'sheets' must be an empty list" \
  "${invalid_list_content_project}"

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
