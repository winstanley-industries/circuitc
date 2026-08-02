#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
  echo "usage: $0 REVISION MODULE LOCK" >&2
  exit 2
fi

revision=$1
module=$2
lock=$3

module_matches=$(grep -c "^[[:space:]]*rev = \"$revision\",$" "$module")
lock_matches=$(grep -c "^[[:space:]]*\"commit\": \"$revision\"$" "$lock")

test "$module_matches" -eq 1
test "$lock_matches" -eq 1
