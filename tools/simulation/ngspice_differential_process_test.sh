#!/bin/sh
set -eu

gate=$1
fake_ngspice=$2
hang_ngspice=$3
fixture=$4
case $fake_ngspice in
  /*) ;;
  *) fake_ngspice=$PWD/$fake_ngspice ;;
esac
case $hang_ngspice in
  /*) ;;
  *) hang_ngspice=$PWD/$hang_ngspice ;;
esac
scratch=$(mktemp -d "${TEST_TMPDIR:-/tmp}/circuitc-ngspice-process.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

set +e
"$gate" "$scratch/missing-ngspice" "$fixture" >"$scratch/missing.stdout" 2>"$scratch/missing.stderr"
missing_status=$?
set -e
test "$missing_status" = 1
test ! -s "$scratch/missing.stdout"
grep -Fx "ngspice 45.2 differential gate failed: host gate unavailable: the selected ngspice executable does not exist" "$scratch/missing.stderr" >/dev/null

printf '%s\n' '#!/bin/sh' 'printf "%s\\n" "** ngspice-45.20 : wrong version"' >"$scratch/wrong-ngspice"
chmod 0700 "$scratch/wrong-ngspice"
set +e
"$gate" "$scratch/wrong-ngspice" "$fixture" >"$scratch/wrong.stdout" 2>"$scratch/wrong.stderr"
wrong_status=$?
set -e
test "$wrong_status" = 1
test ! -s "$scratch/wrong.stdout"
grep -Fx "ngspice 45.2 differential gate failed: host gate requires ngspice 45.2; found 45.20" "$scratch/wrong.stderr" >/dev/null

set +e
"$gate" "$fake_ngspice" "$fixture" >"$scratch/differential.json" 2>"$scratch/differential.stderr"
differential_status=$?
set -e
test "$differential_status" = 1
test ! -s "$scratch/differential.stderr"
grep -F '"ngspice_version": "45.2"' "$scratch/differential.json" >/dev/null
grep -F '"absolute_volts": "9.99999999999999955e-7"' "$scratch/differential.json" >/dev/null
grep -F '"axis_relative": "9.99999999999999980e-13"' "$scratch/differential.json" >/dev/null
grep -F '"pass": 19' "$scratch/differential.json" >/dev/null
grep -F '"fail": 1' "$scratch/differential.json" >/dev/null
test "$(grep -F -c '"status": "pass"' "$scratch/differential.json")" = 19
test "$(grep -F -c '"status": "fail"' "$scratch/differential.json")" = 1

swap_source=$scratch/swap-source
swap_selected=$scratch/swapping-ngspice
printf '%s\n' \
  '#!/bin/sh' \
  'unset PWD SHLVL' \
  "if test \"\$2\" = \"--version\"; then" \
  "  /bin/rm -f \"$swap_selected\"" \
  "  /bin/ln -s \"$fake_ngspice\" \"$swap_selected\"" \
  "  exec /usr/bin/env -i HOME=\"\$HOME\" LANG=\"\$LANG\" LC_ALL=\"\$LC_ALL\" OMP_NUM_THREADS=\"\$OMP_NUM_THREADS\" OPENBLAS_NUM_THREADS=\"\$OPENBLAS_NUM_THREADS\" TMPDIR=\"\$TMPDIR\" TZ=\"\$TZ\" \"$fake_ngspice\" \"\$@\"" \
  'fi' \
  ': > pass-mode' \
  "exec /usr/bin/env -i HOME=\"\$HOME\" LANG=\"\$LANG\" LC_ALL=\"\$LC_ALL\" OMP_NUM_THREADS=\"\$OMP_NUM_THREADS\" OPENBLAS_NUM_THREADS=\"\$OPENBLAS_NUM_THREADS\" TMPDIR=\"\$TMPDIR\" TZ=\"\$TZ\" \"$fake_ngspice\" \"\$@\"" >"$swap_source"
chmod 0700 "$swap_source"
ln -s "$swap_source" "$swap_selected"
set +e
"$gate" "$swap_selected" "$fixture" >"$scratch/swap.json" 2>"$scratch/swap.stderr"
swap_status=$?
set -e
test "$swap_status" = 0
test ! -s "$scratch/swap.stderr"
grep -F '"pass": 20' "$scratch/swap.json" >/dev/null
grep -F '"fail": 0' "$scratch/swap.json" >/dev/null
test "$(readlink "$swap_selected")" = "$fake_ngspice"

hang_wrapper=$scratch/hang-wrapper
hang_sentinel=$scratch/descendant-survived
printf '%s\n' \
  '#!/bin/sh' \
  'unset PWD SHLVL' \
  "if test \"\$2\" = \"--version\"; then" \
  "  exec \"$hang_ngspice\" \"\$@\"" \
  'fi' \
  "\"$hang_ngspice\" --sentinel \"$hang_sentinel\" &" \
  "exec \"$hang_ngspice\" \"\$@\"" >"$hang_wrapper"
chmod 0700 "$hang_wrapper"
set +e
"$gate" "$hang_wrapper" "$fixture" >"$scratch/hang.stdout" 2>"$scratch/hang.stderr"
hang_status=$?
set -e
test "$hang_status" = 1
test ! -s "$scratch/hang.stdout"
grep -Fx "ngspice 45.2 differential gate failed: ngspice exceeded the 5 second wall-clock limit" "$scratch/hang.stderr" >/dev/null
/bin/sleep 3
test ! -e "$hang_sentinel"
