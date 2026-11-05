#!/bin/sh
set -eu
site_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
export XDG_STATE_HOME="$site_root/.cwr-state"
export WORKLOOP_STATE_HOME="$site_root/.cwr-state"
export WORKLOOP_STORAGE_IDENTITY="randallb-com-site-checks"
cwr_bin="$site_root/vendor/cwr/target/debug/cwr"
if [ ! -x "$cwr_bin" ]; then
  echo "Build CWR first: sh scripts/cwr-setup.sh" >&2
  exit 1
fi
trap '"$cwr_bin" --workspace "$site_root/cwr" runtime stop >&2 || true' EXIT HUP INT TERM
cd "$site_root/cwr"
"$cwr_bin" --workspace "$site_root/cwr" actions invoke @site_checks--run--checks
