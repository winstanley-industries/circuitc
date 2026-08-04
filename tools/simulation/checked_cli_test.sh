#!/bin/sh
set -eu

circuitc=$1
fixture=$2
scratch=$(mktemp -d "${TEST_TMPDIR:-/tmp}/circuitc-checked-cli.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

chain_directory() {
  root=$1
  expected=$2
  directories=$(find "$root/simulation" -mindepth 1 -maxdepth 1 -type d | LC_ALL=C sort)
  count=$(printf '%s\n' "$directories" | sed '/^$/d' | wc -l | tr -d ' ')
  test "$count" = "$expected"
  printf '%s\n' "$directories"
}

verify_chain() {
  root=$1
  expected=$2
  chain_directory "$root" "$expected" | while IFS= read -r directory; do
    for name in analysis.spice request.json spice-map.json result.json report.json; do
      test -f "$directory/$name"
    done
    count=$(find "$directory" -type f | wc -l | tr -d ' ')
    test "$count" = 5
  done
}

verify_analysis_kinds() {
  root=$1
  for kind in ac_linear_sweep dc_operating_point transient; do
    test "$(grep -l -F "\"kind\": \"$kind\"" "$root"/simulation/*/request.json | wc -l | tr -d ' ')" = 1
    test "$(grep -l -F "\"analysis_kind\": \"$kind\"" "$root"/simulation/*/result.json | wc -l | tr -d ' ')" = 1
    test "$(grep -l -F "\"analysis_kind\": \"$kind\"" "$root"/simulation/*/report.json | wc -l | tr -d ' ')" = 1
  done
  test "$(grep -l -F '.AC LIN 4 1 4' "$root"/simulation/*/analysis.spice | wc -l | tr -d ' ')" = 1
  test "$(grep -l -F '.OP' "$root"/simulation/*/analysis.spice | wc -l | tr -d ' ')" = 1
  test "$(grep -l -F '.TRAN 125e-3 500e-3 0' "$root"/simulation/*/analysis.spice | wc -l | tr -d ' ')" = 1
}

pass_one=$scratch/pass-one
pass_two=$scratch/pass-two
"$circuitc" compile "$fixture" --output-dir "$pass_one" >"$scratch/pass-one.stdout" 2>"$scratch/pass-one.stderr"
"$circuitc" compile "$fixture" --output-dir "$pass_two" >"$scratch/pass-two.stdout" 2>"$scratch/pass-two.stderr"
test ! -s "$scratch/pass-one.stderr"
test ! -s "$scratch/pass-two.stderr"
verify_chain "$pass_one" 3
verify_chain "$pass_two" 3
verify_analysis_kinds "$pass_one"
verify_analysis_kinds "$pass_two"
test -f "$pass_one/checked_voltage_divider.kicad_sch"
test -f "$pass_one/checked_voltage_divider.spice"
diff -r "$pass_one" "$pass_two"

test "$(grep -l -F '"status": "completed"' "$pass_one"/simulation/*/result.json | wc -l | tr -d ' ')" = 3
test "$(grep -l -F '"status": "pass"' "$pass_one"/simulation/*/report.json | wc -l | tr -d ' ')" = 3

failure_source=$scratch/failing.circuitc
sed 's/checks.vout analysis simulation.dc net VOUT sample scalar expected 5 V/checks.vout analysis simulation.dc net VOUT sample scalar expected 6 V/' "$fixture" >"$failure_source"

for suffix in one slash dot; do
  success_root=$scratch/fail-$suffix
  mkdir -p "$success_root"
  printf '%s\n' sentinel >"$success_root/sentinel.txt"
  output_argument=$success_root
  if test "$suffix" = slash; then
    output_argument=$success_root/
  elif test "$suffix" = dot; then
    output_argument=$success_root/.
  fi
  set +e
  "$circuitc" compile "$failure_source" --output-dir "$output_argument" >"$scratch/fail-$suffix.stdout" 2>"$scratch/fail-$suffix.stderr"
  status=$?
  set -e
  test "$status" = 1
  test "$(find "$success_root" -type f | wc -l | tr -d ' ')" = 1
  grep -Fx sentinel "$success_root/sentinel.txt" >/dev/null
  test ! -e "$success_root/.failed"
  grep -F 'CC-SIM-CHECK-001' "$scratch/fail-$suffix.stderr" >/dev/null
  verify_chain "$success_root.failed" 3
  verify_analysis_kinds "$success_root.failed"
  test "$(grep -l -F '"status": "completed"' "$success_root.failed"/simulation/*/result.json | wc -l | tr -d ' ')" = 3
  test "$(grep -l -F '"status": "fail"' "$success_root.failed"/simulation/*/report.json | wc -l | tr -d ' ')" = 1
  test "$(grep -l -F '"status": "pass"' "$success_root.failed"/simulation/*/report.json | wc -l | tr -d ' ')" = 2
done

diff -r "$scratch/fail-one.failed" "$scratch/fail-slash.failed"
diff -r "$scratch/fail-one.failed" "$scratch/fail-dot.failed"
