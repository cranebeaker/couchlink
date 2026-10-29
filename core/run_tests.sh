#!/usr/bin/env bash
# Runs every test: unit tests for couchlink-core, protocol round trips, and the
# end to end test that starts couchlinkd --dev --stay --virtual-pads 2.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --workspace -- --nocapture
