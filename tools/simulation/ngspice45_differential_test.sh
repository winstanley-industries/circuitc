#!/bin/sh
set -eu

gate=$1
fixture=$2
scratch=$(mktemp -d "${TEST_TMPDIR:-/tmp}/circuitc-ngspice45.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

if test -n "${CIRCUITC_NGSPICE:-}"; then
  ngspice=$CIRCUITC_NGSPICE
elif command -v ngspice >/dev/null 2>&1; then
  ngspice=$(command -v ngspice)
elif test -x /opt/homebrew/bin/ngspice; then
  ngspice=/opt/homebrew/bin/ngspice
elif test -x /usr/local/bin/ngspice; then
  ngspice=/usr/local/bin/ngspice
else
  echo "ngspice 45.2 host gate unavailable: set CIRCUITC_NGSPICE" >&2
  exit 1
fi

"$gate" "$ngspice" "$fixture" >"$scratch/first.json" 2>"$scratch/first.stderr"
"$gate" "$ngspice" "$fixture" >"$scratch/second.json" 2>"$scratch/second.stderr"
test ! -s "$scratch/first.stderr"
test ! -s "$scratch/second.stderr"
cmp "$scratch/first.json" "$scratch/second.json"
grep -F '"format": "circuitc-ngspice-differential/v1"' "$scratch/first.json" >/dev/null
grep -F '"ngspice_version": "45.2"' "$scratch/first.json" >/dev/null
grep -F '"pass": 20' "$scratch/first.json" >/dev/null
grep -F '"fail": 0' "$scratch/first.json" >/dev/null
test "$(grep -F -c '"status": "pass"' "$scratch/first.json")" = 20
