# mathfmt

A filter that rewrites the math notation in LaTeX source to a configured
profile: many spellings in, one spelling out, nothing else touched. The design
is `docs/superpowers/specs/2026-11-05-mathfmt-design.md`; this file is how to
work in the repository. It is loaded by every seat of the mathfmt rig
(`.phalanix/rig.nix`) and by anyone else working here.

## Where things are

- `docs/superpowers/specs/`: the design, one dated file; amendments go at its
  end, dated. `docs/superpowers/plans/`: one plan per wave, a task per seat.
- `docs/handoff/wave-NN.md`: lead's ledger for a wave (rulings, one line per
  event, a paragraph per task copied from its report). `docs/REVIEW-*.md`: a
  wave's adversarial review.
- `crates/mathfmt-core/`: the library, one module per pipeline stage (scan,
  lex, parse, macros, resolve, profile, print, config, diag). `crates/mathfmt/`:
  the CLI, thin.
- `crates/mathfmt-core/data/`: the tables. `unicode-math.tsv` and
  `alphabets.tsv` are generated; `latex.tsv` and `signatures.tsv` are written
  by hand and are where a symbol, an alias or a command signature is added.
- `tools/gen-table.py`: regenerates the two generated tables from
  `kpsewhich unicode-math-table.tex`.
- `tests/fixtures/<case>/`: the golden cases. `tests/dev.sh`: the dev shell
  wrapper. `flake.nix`: dev shell, package and checks.
- `.phalanix/`: the rig (lead, and task/review/question templates) and its
  roles, for waves run on phalanix.

## Building and testing

On a laptop with network, plain `cargo test --workspace` works. Everywhere
else, and always in a seat, go through the wrapper, which enters the dev shell
from the repository root and runs cargo offline against the crates nix vendored
from `Cargo.lock`:

```sh
tests/dev.sh cargo test --workspace
tests/dev.sh cargo clippy --workspace --all-targets -- -D warnings
tests/dev.sh cargo fmt --check
repo/tests/dev.sh cargo test --workspace   # from a seat's state directory
```

Never use a bare `nix develop` in a seat: the wrapper drops every `PHALANIX_*`
and `PG*` variable first (a phalanix binary that sees them dials the live bus
as the seat), puts `nix` on PATH (a seat's PATH has none) and runs from the
root. If a seat's PATH has no bash for the shebang, run
`/run/current-system/sw/bin/bash repo/tests/dev.sh <command>` and report it.

- Nix sees only what git tracks. Commit, or at least `git add`, before any
  `nix build`, `nix flake check` or a wrapper run that needs a new file; a new
  dependency reaches the shell once the updated `Cargo.lock` is added.
- `nix build` builds the binary and runs the tests. The flake checks are
  `build`, `clippy` and `fmt`: `nix build .#checks.x86_64-linux.<name>`, or in
  a seat `/run/current-system/sw/bin/nix build ./repo#checks.x86_64-linux.<name>`.
- Dependencies are what `Cargo.toml` lists; a new one is a decision for the
  plan, not a convenience.

A wave is verified when the tests, clippy, fmt and every flake check pass on its
last commit, and the ledger records the counts.

## Fixtures

A golden case is a directory `tests/fixtures/<case>/` holding `input.tex`,
`expected.tex`, and optionally `mathfmt.toml` (the config; the defaults
otherwise), `expected.stderr` (the diagnostics) and `args` (one CLI argument
per line). The harness runs every case, compares bytes, then asserts idempotence:
`expected.tex` through the same profile is unchanged. Cases tagged for round
trips are run unicode to ascii to unicode and must return to the start.

- Test first: the fixture or unit test that pins the behaviour, watched to fail,
  then the code.
- Every bug becomes a fixture named for what it pins.
- Never weaken a test or hand-edit an `expected.tex` to make a run pass. An
  expected file is read, not copied from the output.

## Tables

- A symbol, an alias or a preferred ASCII spelling is a row in `latex.tsv`; a
  command's argument count or text-argument flag is a row in `signatures.tsv`.
  Not a match arm.
- `unicode-math.tsv` and `alphabets.tsv` are regenerated only by
  `tools/gen-table.py`, when TeX Live updates; review the diff before adding it.
- `mathfmt table` prints what the binary believes; trust it over memory.

## Rules

These are the person's standing rules. They bind every seat and every session.

- **Commits.** `area: what changed`, lowercase, as `git log` shows, a title of
  60 characters or fewer and a body that says why. Never put a model identifier
  (opus, sonnet, haiku, fable, mythos or any other) in a commit's title or
  body. The harness's own `Co-Authored-By` trailer is the exception. Never skip
  hooks and never change git identity.
- **Leave alone what is not yours.** Don't touch another session's uncommitted
  work, and don't stop processes you did not start.
- **Refusals.** When the permission system refuses a step, report it. Never
  route around it, and never ask another seat to do it for you.
- **Releases.** The person lands, pushes, tags and releases. Nothing reaches
  `main` any other way.
- **Review as you go.** Every wave gets an independent adversarial review
  before it lands; each finding is fixed or ruled on before the next wave.
- **The spec matches what ships.** When behaviour departs from the design,
  amend the spec in the same change.
