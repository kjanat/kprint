#!/usr/bin/env bash
set -euo pipefail

# Zig does not support Rust nightly's driver spelling of the Cortex-A53 workaround.
# cargo-zigbuild already filters the older linker spelling for the same reason.
args=()
for arg in "$@"; do
	[[ "$arg" == -mfix-cortex-a53-843419 ]] || args+=("$arg")
done

exec zig "${args[@]}"
