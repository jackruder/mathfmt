#!/usr/bin/env bash
# tests/dev.sh — run a command in the dev shell as a development checkout,
# never as the seat that may be running it.
#
#   tests/dev.sh cargo test --workspace
#   tests/dev.sh cargo clippy --workspace --all-targets -- -D warnings
#   tests/dev.sh cargo fmt --all -- --check
#   repo/tests/dev.sh cargo test --workspace     # from a seat's state directory
#
# A seat's unit exports its identity (PHALANIX_SEAT_ID, PHALANIX_RIG, the live
# bus in PHALANIX_BUS_DSN, PGPASSWORD, ...), and a phalanix binary that sees
# PHALANIX_SEAT_ID takes the seat's bus over any --config. This drops every
# PHALANIX_* and PG* variable before entering the shell. It also puts nix on
# PATH (a seat's PATH has none) and runs the command from the repository root.
#
# The dev shell itself (flake.nix) points cargo at the crates nix vendored
# from Cargo.lock and sets it offline, through a per-checkout CARGO_HOME
# (.cargo-home/, git-ignored), so cargo, cargo clippy and rust-analyzer all
# build with no network. Nix sees only what git tracks, so a new dependency
# reaches the shell once Cargo.lock is added.
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
	"$NIX" develop "$ROOT" --command "$@"
