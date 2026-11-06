#!/usr/bin/env bash
# tests/dev.sh — run a command in the dev shell as a development checkout,
# never as the seat that may be running it.
#
#   tests/dev.sh cargo test --workspace
#   tests/dev.sh cargo clippy --workspace --all-targets -- -D warnings
#   tests/dev.sh cargo fmt --check
#   repo/tests/dev.sh cargo test --workspace     # from a seat's state directory
#
# A seat's unit exports its identity (PHALANIX_SEAT_ID, PHALANIX_RIG, the live
# bus in PHALANIX_BUS_DSN, PGPASSWORD, ...), and a phalanix binary that sees
# PHALANIX_SEAT_ID takes the seat's bus over any --config. This drops every
# PHALANIX_* and PG* variable before entering the shell. It also puts nix on
# PATH (a seat's PATH has none) and runs the command from the repository root.
#
# A cargo command runs against the crates the dev shell vendored from
# Cargo.lock (CARGO_VENDOR_DIR, see flake.nix), offline: a seat's proxy does
# not allow crates.io, and the nix daemon has already fetched every crate.
# Any other command runs as given. Nix sees only what git tracks, so a new
# dependency reaches the shell once Cargo.lock is added.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NIX="$(command -v nix || echo /run/current-system/sw/bin/nix)"

[ "$#" -gt 0 ] || {
	echo "usage: tests/dev.sh <command> [args...]" >&2
	exit 2
}

drop=()
for name in $(compgen -e); do
	case "$name" in
	PHALANIX_* | PG*) drop+=(-u "$name") ;;
	esac
done

cd "$ROOT"
exec env "${drop[@]}" PATH="$PATH:$(dirname "$NIX")" \
	"$NIX" develop "$ROOT" --command bash -c '
		if [ "$1" = cargo ]; then
			shift
			set -- cargo \
				--config "source.crates-io.replace-with=\"vendored\"" \
				--config "source.vendored.directory=\"$CARGO_VENDOR_DIR\"" \
				"$@"
			export CARGO_NET_OFFLINE=true
		fi
		exec "$@"
	' dev.sh "$@"
