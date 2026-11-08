#!/usr/bin/env bash
# Install a built wheel or npm tarballs into a fresh project and record one session with the
# CLI the package bundles; PATH is cut down so no other termtrace can stand in.
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Evidence paths must not pass through a symlink, which a shared Cargo target directory may be.
mkdir -p "$root/.cache"
readonly scratch="$(mktemp -d "$root/.cache/package-test.XXXXXX")"
cleanup() {
    local result=$?
    if [[ "$result" -ne 0 ]]; then
        find "$scratch" -name '*.driver.log' -exec tail -n 40 {} + >&2 || true
    fi
    rm -rf "$scratch"
}
trap cleanup EXIT
unset TERMTRACE_BIN
readonly isolated_path=/usr/bin:/bin

test_python() {
    local wheel=$1
    uv venv --quiet "$scratch/venv"
    uv pip install --quiet --python "$scratch/venv/bin/python" "$wheel"
    PATH=$isolated_path "$scratch/venv/bin/python" - "$scratch/evidence" <<'PY'
import sys

from termtrace import Termtrace

tt = Termtrace(evidence_root=sys.argv[1])
with tt.record(["/bin/sh", "-c", "echo PACKAGE_OK; sleep 5"], rows=24, cols=80) as rec:
    rec.screen().wait_for_text("PACKAGE_OK", timeout_secs=10.0)
print("python package records with its bundled CLI")
PY
}

test_node() {
    local main=$1 platform=$2 node
    node=$(command -v node)
    npm init --yes >/dev/null
    npm install --no-audit --no-fund "$platform" "$main"
    PATH=$isolated_path "$node" - "$scratch/evidence" <<'JS'
const { Termtrace } = require("@zigai/termtrace");

(async () => {
    const tt = new Termtrace(null, process.argv[2]);
    const rec = await tt.launch(["/bin/sh", "-c", "echo PACKAGE_OK; sleep 5"]);
    try {
        await rec.screen().waitForText("PACKAGE_OK", 10000);
    } finally {
        await rec.stop();
    }
    console.log("npm package records with its bundled CLI");
})().catch((error) => {
    console.error(error);
    process.exit(1);
});
JS
}

case "${1:-}" in
    python) test_python "$(realpath "$2")" ;;
    node)
        main=$(realpath "$2")
        platform=$(realpath "$3")
        cd "$scratch"
        test_node "$main" "$platform"
        ;;
    *)
        echo "usage: $0 python <wheel> | $0 node <main.tgz> <platform.tgz>" >&2
        exit 2
        ;;
esac
