#!/usr/bin/env bash
# tests/pre-commit-e2e.sh — the collaborator's path, end to end: build the
# wheel, then run mathfmt as a `repo: local`, `language: python` pre-commit
# hook whose dependency is that wheel, in a scratch git repository. Needs the
# dev shell (for maturin) and `uvx` with network for pre-commit itself, so it
# is run by hand, not by `cargo test`.
#
#   tests/pre-commit-e2e.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/mathfmt-pre-commit.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

"$ROOT/tests/dev.sh" maturin build --release --out "$WORK/wheels" >/dev/null
WHEEL="$(ls "$WORK"/wheels/mathfmt_tex-*.whl)"

mkdir "$WORK/paper"
cd "$WORK/paper"
git init -q
git config user.email test@example.invalid
git config user.name test
cat >.pre-commit-config.yaml <<YAML
repos:
  - repo: local
    hooks:
      - id: mathfmt
        name: mathfmt
        entry: mathfmt
        language: python
        additional_dependencies: ["$WHEEL"]
        args: ["--write", "--profile", "ascii"]
        files: \.(tex|sty)$
YAML
printf '%s\n' '\documentclass{article}' '\usepackage{amssymb}' '\begin{document}' \
	'Let $x ∈ ℝ$ and $α ≤ β$.' '\end{document}' >main.tex
git add -A
export PRE_COMMIT_HOME="$WORK/cache"

if uvx --quiet pre-commit run --all-files; then
	echo "pre-commit-e2e: the first run should have rewritten main.tex" >&2
	exit 1
fi
grep -qF 'Let $x \in \mathbb{R}$ and $\alpha \leq \beta$.' main.tex || {
	echo "pre-commit-e2e: main.tex was not rewritten as expected:" >&2
	cat main.tex >&2
	exit 1
}
uvx --quiet pre-commit run --all-files
echo "pre-commit-e2e: the hook rewrote main.tex, then passed"
