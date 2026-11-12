#!/usr/bin/env bash
# Every crate published to crates.io must ship the LICENSE file, and its
# manifest must point at that file instead of claiming a plain SPDX licence:
# the project is AGPL v3 with the Commons Clause, which no SPDX id expresses.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

status=0
for crate in rustydemon-blp2 rustydemon-gr2 rustydemon-lib; do
    if ! cargo package -p "$crate" --list --allow-dirty 2>/dev/null | grep -qx 'LICENSE'; then
        echo "FAIL $crate: LICENSE is not in the package" >&2
        status=1
    fi
    meta=$(cargo metadata --format-version 1 --no-deps)
    if ! python3 -c '
import json, sys
crate = sys.argv[1]
pkg = next(p for p in json.load(sys.stdin)["packages"] if p["name"] == crate)
sys.exit(0 if pkg["license_file"] and not pkg["license"] else 1)
' "$crate" <<<"$meta"; then
        echo "FAIL $crate: manifest must set license-file and no license" >&2
        status=1
    fi
done

if ! grep -q '^"Software" means Rusty Demon' LICENSE; then
    echo 'FAIL LICENSE: the Commons Clause must name Rusty Demon as the Software' >&2
    status=1
fi

[ "$status" -eq 0 ] && echo "ok: crate licence metadata"
exit "$status"
