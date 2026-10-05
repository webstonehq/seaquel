#!/usr/bin/env bash
# Notarizes signed macOS command-line binaries (the DuckDB helper, seaquel-cli,
# seaquel-tui) so a copy downloaded in a browser and moved into place by hand
# passes Gatekeeper. The app downloads its own copies with no quarantine flag;
# this is for the ones people fetch from the release page themselves.
#
#   scripts/notarize-macos.sh <file> [<file> ...]
#
# Each file must already be signed with the hardened runtime and a secure
# timestamp (`codesign --options runtime --timestamp`). Several files are
# submitted at once and waited for together. Exits non-zero unless every
# submission comes back Accepted, printing `notarytool log` for any that
# doesn't.
#
# A bare Mach-O binary can't be stapled (`stapler` takes only apps, packages
# and disk images), so the ticket stays with Apple and Gatekeeper fetches it
# online on first run. Notarizing doesn't change the file's bytes, so a
# SHA-256 taken after this (the helper's pin) still matches.
#
# Credentials are the ones tauri-action notarizes the app with:
#   APPLE_ID        the Apple ID
#   APPLE_PASSWORD  an app-specific password for it
#   APPLE_TEAM_ID   the team
# NOTARIZE_TIMEOUT bounds the wait per file (notarytool's --timeout, default 30m).
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <file> [<file> ...]" >&2
  exit 2
fi
for var in APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
  if [ -z "${!var:-}" ]; then
    echo "notarize: $var isn't set" >&2
    exit 2
  fi
done
for file in "$@"; do
  if [ ! -f "$file" ]; then
    echo "notarize: $file isn't a file" >&2
    exit 2
  fi
done

timeout="${NOTARIZE_TIMEOUT:-30m}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

auth=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")

# One JSON field of notarytool's answer, or nothing.
field() {
  plutil -extract "$1" raw -o - - <"$2" 2>/dev/null || true
}

notarize_one() {
  local file="$1" n="$2"
  local name zip out id status rc sig
  name="$(basename "$file")"

  # Notarization refuses a signature without a secure timestamp; say so
  # before spending minutes on the round trip.
  sig="$(codesign -dvv "$file" 2>&1 || true)"
  if ! grep -q '^Timestamp=' <<<"$sig"; then
    echo "$name: not signed with a secure timestamp (codesign --timestamp)"
    return 1
  fi

  zip="$work/$n/$name.zip"
  mkdir -p "$work/$n"
  ditto -c -k --keepParent "$file" "$zip"

  out="$work/$n/submit.json"
  rc=0
  xcrun notarytool submit "$zip" "${auth[@]}" --wait --timeout "$timeout" \
    --output-format json >"$out" 2>"$work/$n/submit.err" || rc=$?
  id="$(field id "$out")"
  if [ -z "$id" ]; then
    # A wait that timed out may answer in text; its id is the first UUID.
    id="$(grep -Eoh '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' "$out" "$work/$n/submit.err" | head -n 1 || true)"
  fi
  status="$(field status "$out")"
  echo "$name: submission ${id:-(none)}, status ${status:-(none)}, notarytool exit $rc"

  if [ "$status" = "Accepted" ] && [ "$rc" -eq 0 ]; then
    return 0
  fi
  if [ -s "$work/$n/submit.err" ]; then
    sed "s/^/$name: /" "$work/$n/submit.err"
  fi
  if [ -z "$status" ] || [ "$status" = "In Progress" ]; then
    echo "$name: no answer within $timeout"
  fi
  if [ -n "$id" ]; then
    echo "$name: notarytool log $id:"
    xcrun notarytool log "$id" "${auth[@]}" 2>&1 | sed "s/^/$name: /" || true
  fi
  return 1
}

# Each file in the background, its output kept apart and printed in order.
pids=()
n=0
for file in "$@"; do
  n=$((n + 1))
  notarize_one "$file" "$n" >"$work/out.$n" 2>&1 &
  pids+=("$!")
done

failed=0
n=0
for pid in "${pids[@]}"; do
  n=$((n + 1))
  if ! wait "$pid"; then
    failed=$((failed + 1))
  fi
  cat "$work/out.$n"
done

if [ "$failed" -ne 0 ]; then
  echo "notarize: $failed of $# not accepted" >&2
  exit 1
fi
echo "notarize: $# accepted (not stapled: a bare binary can't be; Gatekeeper checks online)"
