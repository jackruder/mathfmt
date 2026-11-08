# mathfmt

`mathfmt` rewrites the mathematical notation inside LaTeX source so that one
symbol is spelled one way, the way your profile says. `\R`, `\mathbb{R}`,
`\symbb{R}` and `ℝ` are four spellings of one symbol; a run makes them one. It
is a filter: LaTeX in, the same LaTeX out, with only the recognized math tokens
changed and every other byte untouched.

It is not a formatter. Whitespace, line breaks and indentation are left to
`latexindent`, `tex-fmt` or `badness`, and `mathfmt` runs beside them. It has no
opinion about when it runs (a pre-commit hook, a CI check, an editor's
format-on-save, a one-off switch of a whole repository to ASCII for a venue
that cannot take Unicode) or about what compiles the result.

Status: version zero. Atomic symbols (named symbols, Greek letters, styled
letters and digits, zero-argument macros that stand for one of them) convert in
both directions; parameterized notations are designed and not yet built. See
`docs/superpowers/specs/2026-11-05-mathfmt-design.md`.

## Install

```sh
cargo install --path .                      # any platform with a Rust toolchain
nix build                                   # Linux and macOS; ./result/bin/mathfmt
nix profile install .                       # or add the flake's package to your configuration
```

## Use

```
mathfmt [OPTIONS] [FILE...]
    no FILE: read stdin, write stdout
    -p, --profile NAME        profile from the config (default: config's `profile`)
    -c, --config PATH         config file (default: nearest mathfmt.toml)
        --macros FILE         an extra definition source (repeatable)
    -w, --write               rewrite FILEs in place (atomic: temp file + rename)
        --check               exit 1 if any file would change; print their names
        --diff                print a unified diff per changed file
        --emit KIND           regions | tokens | atoms | symbols, as JSON lines
        --format text|json    diagnostics format (default text)
        --strict              warnings make the exit status 1
    -j, --jobs N              parallel files (default: available cores)
    -q, --quiet               no diagnostics on stderr
    -v, --verbose             also show informational diagnostics

mathfmt rename --from SPELLING --to SPELLING [same options] [FILE...]
mathfmt table [--profile NAME]
```

```sh
mathfmt < draft.tex > out.tex
mathfmt --profile unicode --write sections/*.tex
mathfmt --check --diff main.tex sections/*.tex
mathfmt --emit symbols main.tex | jq -r .atom | sort | uniq -c
mathfmt rename --from 𝔉 --to ℱ --write sections/*.tex
```

Exit status: 0 clean; 1 `--check` found changes or `--strict` found warnings;
2 usage, config or I/O error. Silent on success. With no FILE it reads stdin,
so guard a shell expansion that may come back empty; a pre-commit hook passes
the files itself.

A project's `mathfmt.toml` names the definition files whose macros the tool
reads and the profiles it may emit; two profiles, `unicode` and `ascii`, exist
with defaults even when the file does not mention them.

## What it does to a real paper

Run read-only over the eight `.tex` files of an ICML manuscript that defines
`\R`, `\E`, `\N` and friends in its style file:

```sh
mathfmt --check --diff --macros ddssm.sty main.tex sections/*.tex
```

| | |
|---|---|
| time for all eight files | about 25 ms |
| symbol occurrences rewritten | 679 |
| distinct symbols | 48 |
| warnings | 0 |
| style groups left as written | 0 |

The result taken through `--profile ascii` and back to `unicode` returns seven
of the eight files byte for byte. The eighth writes `\mathbf{\sigma_\theta}`,
which pdflatex renders as plain italic (its `\mathbf` ignores lowercase Greek);
mathfmt reads it literally as bold upright `𝛔_{𝛉}`, and the `ascii` profile
spells bold Greek with `\boldsymbol`, which reads back as bold italic `𝝈_{𝜽}`.
A profile with `bf_greek = "\\mathbf"` keeps pdflatex's reading and closes the
round trip; see "Known collapses" below. A line of the diff:

```diff
-Let \( \mathbf{x}_{1:T} = (\mathbf{x}_1, \dots, \mathbf{x}_T) \) with \( \mathbf{x}_t \in \mathbb{R}^D \)
+Let \( 𝐱_{1:T} = (𝐱_1, \dots, 𝐱_T) \) with \( 𝐱_t ∈ ℝ^D \)
```

The `ascii` profile on the same files makes 26 changes, every one a macro
inlined to its definition: `\N` to `\mathcal{N}`, `\E` to `\mathbb{E}`.
Collaborators who prefer the macros write a profile with
`spellings."ℝ" = "\\R"` and get them back.

## As a pre-commit hook

```yaml
repos:
  - repo: ssh://forgejo@git.buttermilks.rocks/jack/mathfmt.git
    rev: v0.1.0
    hooks:
      - id: mathfmt          # rewrites staged .tex/.sty files to the profile
      # - id: mathfmt-check  # or: fail the commit, change nothing
```

## Known collapses

Two places where the Unicode pivot loses a distinction, by design and
configurable:

- **cal and scr.** `\mathcal{F}` and `\mathscr{F}` both print as the one Unicode
  script alphabet (`𝓕`); reading it back gives `\mathcal` unless the profile
  sets `script_reads_as = "scr"`.
- **bf_greek.** The `ascii` profile prints a bold Greek letter as
  `\boldsymbol{\alpha}` because pdflatex's `\mathbf` renders nothing bold for
  Greek. Bold upright `𝛂` and bold italic `𝜶` therefore both become
  `\boldsymbol{\alpha}` and come back as `𝜶`. Set `style_commands.bf_greek =
  "\\mathbf"` for the literal reading.

## Developing

`cargo test --workspace` on a laptop; `tests/dev.sh cargo test --workspace`
anywhere else (it enters the dev shell and runs cargo offline against the
crates nix vendored from `Cargo.lock`). `CLAUDE.md` has the rest: the fixture
discipline, the tables, and the rules.
