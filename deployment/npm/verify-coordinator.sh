#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd -- "${script_dir}/../.." && pwd)"
source_checkout="${1:?Usage: verify-coordinator.sh BEEPBOOP_CHECKOUT}"
bun_executable="${BUN:-bun}"
pin="$(sed -nE 's/.*uses: kjanat\/beepboop\/actions\/publish@([a-f0-9]{40}).*/\1/p' "${root}/.github/workflows/publish.yml")"
[[ "${pin}" =~ ^[a-f0-9]{40}$ ]] || {
	echo 'Expected one pinned Beepboop publication action' >&2
	exit 1
}
fixture="$(mktemp -d)"
trap 'rm -rf -- "${fixture}"' EXIT

git -C "${source_checkout}" archive "${pin}" | tar -xf - -C "${fixture}"
patch --batch --forward -d "${fixture}" -p1 <"${script_dir}/beepboop-contract.patch"
cd -- "${fixture}"
"${bun_executable}" install --frozen-lockfile --ignore-scripts
"${bun_executable}" test test/flows/publish-ledger.test.ts
