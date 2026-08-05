#!/usr/bin/env bash
# Genesis for the allod repo's own governance graph (run once, by the
# owner, from the repo root). Creates .allod/ (keys stay local,
# gitignored) and installs governance/policy.yaml.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if [ -d .allod ]; then
  echo ".allod already exists — refusing to re-run genesis" >&2
  exit 1
fi

cargo run -q -p allod -- init . --owner conner --schema ontologies
cargo run -q -p allod -- install-policy . governance/policy.yaml --as conner
cargo run -q -p allod -- verify .

echo
echo "Genesis complete. Commit .allod/ (keys/ is gitignored)."
