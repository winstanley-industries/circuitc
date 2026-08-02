#!/usr/bin/env bash
set -euo pipefail

tool=$1
fake_gh=$2
temporary_directory=$(mktemp -d)
trap 'rm -rf "$temporary_directory"' EXIT

help_output="$temporary_directory/help.txt"
"$tool" --help >"$help_output"
grep -F -- "--repo REPO" "$help_output" >/dev/null
grep -F -- "--pr PR" "$help_output" >/dev/null
grep -F -- "--json" "$help_output" >/dev/null

python3_executable=$(command -v python3)
missing_bin="$temporary_directory/missing-bin"
mkdir "$missing_bin"
ln -s "$python3_executable" "$missing_bin/python3"

set +e
PATH="$missing_bin" "$tool" --repo owner/repo --pr 3 \
  >"$temporary_directory/missing.stdout" \
  2>"$temporary_directory/missing.stderr"
missing_status=$?
set -e

test "$missing_status" -eq 2
grep -F "error: gh CLI not available" "$temporary_directory/missing.stderr" >/dev/null

fake_bin="$temporary_directory/fake-bin"
mkdir "$fake_bin"
ln -s "$python3_executable" "$fake_bin/python3"
cp "$fake_gh" "$fake_bin/gh"
chmod +x "$fake_bin/gh"

PATH="$fake_bin" "$tool" --repo owner/repo --pr 3 \
  >"$temporary_directory/human.stdout"
grep -F "owner/repo#3 head head-1" "$temporary_directory/human.stdout" >/dev/null
grep -F "threads total=1 resolved=0 unresolved=1 current=1 outdated=0" \
  "$temporary_directory/human.stdout" >/dev/null
grep -F -- "- current [current] src/current.rs:7 @reviewer" \
  "$temporary_directory/human.stdout" >/dev/null
