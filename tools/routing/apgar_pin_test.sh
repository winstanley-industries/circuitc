#!/bin/sh
set -eu

if [ "$#" -ne 4 ]; then
  echo "usage: $0 REVISION MODULE RUST_MODULE ADAPTER" >&2
  exit 2
fi

revision=$1
module=$2
rust_module=$3
adapter=$4

module_matches=$(grep -c "^[[:space:]]*commit = \"$revision\",$" "$module")
rust_matches=$(grep -c "PINNED_APGAR_SOURCE_REVISION: &str = \"$revision\";" "$rust_module")
adapter_matches=$(grep -c "kSourceRevision = \"$revision\";" "$adapter")

test "$module_matches" -eq 1
test "$rust_matches" -eq 1
test "$adapter_matches" -eq 1
