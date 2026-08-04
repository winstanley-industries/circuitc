#!/bin/sh
set -eu

circuitc=$1
fixture=$2
scratch=$(mktemp -d "${TEST_TMPDIR:-/tmp}/circuitc-routing-cli.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

verify_routing_chain() {
  root=$1
  directories=$(find "$root/routing" -mindepth 1 -maxdepth 1 -type d | LC_ALL=C sort)
  count=$(printf '%s\n' "$directories" | sed '/^$/d' | wc -l | tr -d ' ')
  test "$count" = 1
  directory=$(printf '%s\n' "$directories")
  for name in request.json result.json projection.json; do
    test -f "$directory/$name"
  done
  test "$(find "$directory" -type f | wc -l | tr -d ' ')" = 3
  grep -F '"kind":"completed"' "$directory/result.json" >/dev/null
  request_sha=$(grep -o '"request_sha256":"[0-9a-f]\{64\}"' "$directory/result.json" | cut -d '"' -f 4)
  test -n "$request_sha"
  grep -F "\"request_sha256\":\"$request_sha\"" "$directory/projection.json" >/dev/null
  selected=$(grep -o '"selected_candidate_id":"[0-9a-f]\{32\}"' "$directory/result.json" | head -1 | cut -d '"' -f 4)
  test -n "$selected"
  grep -F "\"selected_candidate_id\":\"$selected\"" "$directory/projection.json" >/dev/null
}

first=$scratch/first
second=$scratch/second
if ! "$circuitc" compile "$fixture" --output-dir "$first" >"$scratch/first.stdout" 2>"$scratch/first.stderr"; then
  cat "$scratch/first.stderr" >&2
  exit 1
fi
if ! "$circuitc" compile "$fixture" --output-dir "$second" >"$scratch/second.stdout" 2>"$scratch/second.stderr"; then
  cat "$scratch/second.stderr" >&2
  exit 1
fi
test ! -s "$scratch/first.stderr"
test ! -s "$scratch/second.stderr"
verify_routing_chain "$first"
verify_routing_chain "$second"
test -f "$first/routed_voltage_divider.kicad_pcb"
test -f "$first/routed_voltage_divider.kicad-map.json"
grep -F '(segment' "$first/routed_voltage_divider.kicad_pcb" >/dev/null
grep -A 8 '"semantic_path": "board.autoroute.vout.segment.00000000"' \
  "$first/routed_voltage_divider.kicad-map.json" | grep -F '"location": {' >/dev/null
diff -r "$first" "$second"

preflight=$scratch/preflight-output
external=$scratch/preflight-external
mkdir -p "$preflight" "$external"
printf '%s\n' 'board sentinel' >"$preflight/routed_voltage_divider.kicad_pcb"
printf '%s\n' 'external sentinel' >"$external/sentinel.txt"
ln -s "$external" "$preflight/routing"
set +e
"$circuitc" compile "$fixture" --output-dir "$preflight" >"$scratch/preflight.stdout" 2>"$scratch/preflight.stderr"
preflight_status=$?
set -e
test "$preflight_status" = 3
grep -Fx 'board sentinel' "$preflight/routed_voltage_divider.kicad_pcb" >/dev/null
grep -Fx 'external sentinel' "$external/sentinel.txt" >/dev/null
test -L "$preflight/routing"
test "$(find "$preflight" -type f | wc -l | tr -d ' ')" = 1
grep -F 'CC-CLI-IO-002' "$scratch/preflight.stderr" >/dev/null

failure_source=$scratch/failing.circuitc
sed 's/clearance 0.2 mm/clearance 9 mm/' "$fixture" >"$failure_source"
output=$scratch/failure-output
mkdir -p "$output"
printf '%s\n' sentinel >"$output/sentinel.txt"
set +e
"$circuitc" compile "$failure_source" --output-dir "$output" >"$scratch/failure.stdout" 2>"$scratch/failure.stderr"
status=$?
set -e
test "$status" = 1
test "$(find "$output" -type f | wc -l | tr -d ' ')" = 1
grep -Fx sentinel "$output/sentinel.txt" >/dev/null
test ! -e "$output.failed"
grep -E 'CC-ROUTE-(IMPORT|PROCESS)-00[1-9]' "$scratch/failure.stderr" >/dev/null
