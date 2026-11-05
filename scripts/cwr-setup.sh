#!/bin/sh
set -eu
site_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cargo build --locked --manifest-path "$site_root/vendor/cwr/Cargo.toml" -p cwr-cli -p cwp-runtime -p cwr-adapters --bin cwr --bin cwp-runtime --bin cwr-adapter-sh
