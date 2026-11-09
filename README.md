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
        --project             FILEs are root documents: take every file each one
                              inputs (no FILE: --root, else the config's `root`)
        --root FILE           the root document of the FILEs: its preamble's
                              packages and definitions apply to them
    -w, --write               rewrite FILEs in place (atomic: temp file + rename)
        --check               exit 1 if any file would change; print their names
        --diff                print a unified diff per changed file
        --emit KIND           regions | tokens | atoms | symbols, as JSON lines
        --verify              compile before and after, compare the rendered pages
        --engine PROGRAM      the TeX engine --verify uses (default: the config's
                              `engine`, else what the root's preamble says,
                              else pdflatex)
        --declare             under --verify, add the lines of `mathfmt
                              declarations` to a document's preamble, both sides
        --preamble FILE       preamble for snippets under --verify
        --dpi N               resolution of the page comparison (default 100)
        --keep DIR            keep the --verify build trees
        --format text|json    diagnostics format (default text)
        --strict              warnings make the exit status 1
    -j, --jobs N              parallel files (default: available cores)
    -q, --quiet               no diagnostics on stderr
    -v, --verbose             also show informational diagnostics

mathfmt rename --from SPELLING --to SPELLING [same options] [FILE...]
mathfmt declarations [--project] [--root FILE] [--with MECHANISM] [FILE...]
mathfmt project [ROOT]
mathfmt table [--profile NAME]
```

```sh
mathfmt < draft.tex > out.tex
mathfmt --profile unicode --write sections/*.tex
mathfmt --check --diff main.tex sections/*.tex
mathfmt --emit symbols main.tex | jq -r .atom | sort | uniq -c
mathfmt rename --from 𝔉 --to ℱ --write sections/*.tex
mathfmt --verify --macros ddssm.sty main.tex sections/*.tex
mathfmt --project main.tex --write
mathfmt --project main.tex --verify --declare
mathfmt declarations --project main.tex >> preamble.tex
```

Exit status: 0 clean; 1 `--check` found changes, `--strict` found warnings or
`--verify` found a difference; 2 usage, config or I/O error. Silent on
success. With no FILE it reads stdin, so guard a shell expansion that may come
back empty; a pre-commit hook passes the files itself.

### A whole paper at once

A paper is a root file, the files it reads with `\input` and `\include`, and a
preamble that loads packages, some of them local. Name the root and mathfmt
reads that much itself, without running TeX:

```sh
mathfmt project main.tex            # what it found: files, packages, engine
mathfmt --project main.tex --check  # every file the root inputs
```

or once, in `mathfmt.toml` beside the root:

```toml
root = "main.tex"
```

With a root, named either way, the definitions of its preamble and of the
local packages and classes it loads (`\usepackage{ddssm}` with a `ddssm.sty`
beside the root) are in force for every file, so a pre-commit hook that is
handed `sections/method.tex` alone still knows the paper's `\R`. `--project`
also takes the file list from the root: the root, then each file its body
inputs, in reading order. A package is read and never rewritten. The reader
does not evaluate conditionals, skips a file name built by a macro, and knows
a distributed package by its name only.

The preamble also says what the document is written for. One that loads
`unicode-math` is compiled with lualatex by `--verify`, and a profile's
commands for the math alphabets become unicode-math's (`\symbf`, `\symbb`,
`\symcal`) instead of `\mathbf`, `\mathbb`, `\mathcal`, which that package
typesets differently; `style_preset = "latex"` or `"unicode-math"` in a
profile decides it by hand, `engine = "…"` in the config names the program.

`mathfmt declarations` prints what the preamble must hold for the notation to
compile in either form: the packages the profile's commands come from that
the preamble does not load, and a meaning for the Unicode characters, each
declared as exactly what the profile spells it with commands.

```
$ mathfmt declarations --project main.tex
% mathfmt declarations: main.tex, profile unicode, unicode-math-input
% packages the notation's commands need and the preamble does not load
\usepackage{unicode-math-input}
% styled letters, as the profile spells them (…)
\umiDeclareMathChar{𝐱}{\mathbf{x}}
\umiDeclareMathChar{𝛔}{\boldsymbol{\sigma}}
\umiDeclareMathChar{𝟙}{\mathbbm{1}}
…
```

How a document reads Unicode is a choice (`--with`, or `unicode_input` in the
config):

| | |
|---|---|
| `unicode-math-input` | pdflatex with that package. It knows every symbol, and reads a character after `\big` or as a script. Only styled letters are declared, where it follows pdflatex's limits. The default under pdflatex. |
| `kernel` | pdflatex with `\DeclareUnicodeCharacter` and no package. Every character is declared, inside `\ensuremath`, so it also works in text. `\big⟨` does not compile. |
| `unicode-math` | lualatex or xelatex with unicode-math. The characters are native; nothing is declared. |

### Proving a conversion is safe

The promise is the same PDF from different source, and `--verify` checks it
rather than arguing it: the original and the converted source are compiled
with the same engine, both PDFs are rasterized with `pdftoppm`, and the pages
are compared pixel for pixel. Text extraction cannot tell a bold `x` from a
plain one; pixels can. A FILE with `\documentclass` is a document and is
compiled as it is, under its own preamble, with the converted copies of the
other FILEs in place:

```
$ mathfmt --project main.tex --verify
broken: main.tex: the converted source does not compile
  ! LaTeX Error: Unicode character 𝐱 (U+1D431)
                 not set up for use with LaTeX.
  the preamble does not read this character: `mathfmt declarations` prints
  the lines it needs, and --verify --declare compiles with them
```

That is the honest answer for a pdflatex paper whose preamble does not read
Unicode math: it is not safe to convert yet. `--declare` answers the next
question without touching the paper: what would it render as with the lines
of `mathfmt declarations` in its preamble? They are added before
`\begin{document}` on both sides.

```
$ mathfmt --project main.tex --verify --declare
differs: main.tex: 1 of 14 pages
  page 2: 3225 pixels in the box x=95..401 y=212..241
  the extracted text is the same; the difference is in glyph shapes or positions
  with 28 declaration lines added to the preamble on both sides
```

One page, and the reason is the paper's: it writes `\mathbf{\sigma_\theta}`,
which pdflatex draws plain, and the Unicode `𝛔` is declared bold. With
`bf_greek = "\\mathbf"` in the profile, which keeps pdflatex's reading, the
verdict is `verified: main.tex (14 pages identical)`, in about eight seconds
for the seventeen files.

A FILE without `\documentclass` is a snippet and is wrapped in a preamble
(`--preamble FILE`, or a built-in one that loads the usual packages and
unicode-math-input and declares the styled letters the snippet uses to mean
what the profile spells them as). `--verify` needs the engine and
`pdftoppm` on PATH; the test suite runs it over every fixture and over the
small projects in `tests/projects/`, one of them under lualatex.

A project's `mathfmt.toml` names its root, the definition files whose macros
the tool reads beyond the root's, and the profiles it may emit; two profiles,
`unicode` and `ascii`, exist with defaults even when the file does not mention
them.

### Notations with arguments

A symbol with parameters has spellings too. Declare the notation once, with
every form the project has ever written, and let each profile pick the one it
emits:

```toml
[notations.KL]
macro    = "\\KL{#1}{#2}"
expanded = "D_{\\mathrm{KL}}(#1 \\| #2)"
spaced   = "D_{\\mathrm{KL}}(#1 \\,\\|\\, #2)"

[profiles.ascii.notations]
KL = "macro"            # D_{\mathrm{KL}}(p \| q)  →  \KL{p}{q}
[profiles.unicode.notations]
KL = "expanded"         # \KL{p}{q}  →  D_{\mathrm{KL}}(p ‖ q)
```

Matching is on the math tree modulo symbols, so `\|`, `\Vert` and `‖` are one
thing, `D_\mathrm{KL}` is `D_{\mathrm{KL}}`, and the holes bind whatever sits
between the literals: `D_{\mathrm{KL}}(\mathcal{N}(\mu,\Sigma) ‖ q_\phi)`
becomes `\KL{\mathcal{N}(\mu, \Sigma)}{q_\phi}`, with the holes converted and
nested notations handled inside them. The form a profile writes goes through
the profile's symbol spellings too, so one form serves both. A profile that
names no form leaves the notation as written; `--emit symbols` still counts
it, form by form.

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

Places where the Unicode pivot loses a distinction, by design and
configurable:

- **cal and scr.** `\mathcal{F}` and `\mathscr{F}` both print as the one Unicode
  script alphabet (`𝓕`); reading it back gives `\mathcal` unless the profile
  sets `script_reads_as = "scr"`.
- **bf_greek.** The `ascii` profile prints a bold Greek letter as
  `\boldsymbol{\alpha}` because pdflatex's `\mathbf` renders nothing bold for
  Greek. Bold upright `𝛂` and bold italic `𝜶` therefore both become
  `\boldsymbol{\alpha}` and come back as `𝜶`. Set `style_commands.bf_greek =
  "\\mathbf"` for the literal reading.
- **ℑ and ℜ.** Unicode has one codepoint each for the fraktur I and R and for
  the imaginary and real parts, so `\mathfrak{I}` and `\Im` are one atom. The
  `ascii` profile spells it `\Im`; pdflatex draws `\mathfrak{I}` from eufm and
  `\Im` from cmsy, and they look different. Set `spellings."frak:I" =
  "\\mathfrak{I}"` (and `frak:R`) for a paper that means the letter.

- **Shape under unicode-math.** With that package's default options the
  shape of a bold or sans letter is the document's (`bold-style`,
  `sans-style`), not the character's: a literal `𝛔`, a literal `𝝈` and
  `\symbf{\sigma}` all draw the house bold sigma, and a literal `𝒙` draws
  upright. The preset therefore spells `bf` as `\symbf` and `sf` as `\symsf`.
  A letter whose shape contradicts the house style (`𝒙`, `𝘈`,
  `\symbfup{\sigma}`, `\symbfit{x}`) changes when it crosses between a
  character and a command. A document that sets `bold-style=literal` and
  `sans-style=literal` wants `bf = "\\symbfup"`, `sf = "\\symsfup"` and
  `bfsf = "\\symbfsfup"` in `style_commands`.

`\mathbb{1}` is not a collapse but a repair: amsfonts has no blackboard digits
and `\mathbb{1}` is the glyph ⊮, so the `bb_digit` key spells `𝟙` as
`\mathbbm{1}`. `--verify` reports each of these as a difference when a
conversion crosses one, which is how they were found.

## Developing

`cargo test --workspace` on a laptop; `tests/dev.sh cargo test --workspace`
anywhere else (it enters the dev shell and runs cargo offline against the
crates nix vendored from `Cargo.lock`). `CLAUDE.md` has the rest: the fixture
discipline, the tables, and the rules.
