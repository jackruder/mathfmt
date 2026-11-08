# mathfmt: design

Date: 2026-11-05. Status: approved in conversation; version zero built from this
document. Amendments go at the end, dated.

## 1. Purpose

`mathfmt` rewrites the mathematical notation inside LaTeX source so that one
symbol is spelled one way, the way a configured *profile* says. It is a
source-to-source filter: LaTeX in, the same LaTeX out, with only the recognized
math tokens changed. It has no opinion about when it runs (a pre-commit hook, a
CI check, an editor's format-on-save, a one-off repository switch) or about what
compiles the result.

The motivating cases, all from one collaborative paper repository:

- Three authors spell the real numbers `\R`, `\mathbb{R}` and `ℝ`. The hook
  makes the committed source one of them.
- Author A writes Unicode (`𝛂 ∈ ℝ^d`) because it reads well in diffs and
  editors. Author B finds that unreadable and wants `\boldsymbol{\alpha}
  \in \mathbb{R}^d` in their working copy. Both are profiles of the same tool.
- A venue's style file cannot take Unicode in math. One profile switch lowers
  every file to ASCII macros; the reverse switch brings them back.
- The authors decide that the set of sigma-fields should be `ℱ` rather than
  `𝔉`. One rename changes every instance, whatever its spelling was.

## 2. Scope of version zero, and what is designed but deferred

Version zero converts **atomic symbols**: single glyphs, Greek letters, named
symbols (`\in`, `\leq`, `\sum`), styled letters and digits (`\mathbf{x}`,
`\mathbb{R}`, `\mathcal{N}`, `𝐱`, `ℝ`, `𝒩`), and zero-argument user macros
whose expansion is such a symbol (`\R`, `\E`). It reads user macro definitions
so collaborators may keep writing macros, and it emits macros when a profile
asks for them.

Designed now, built later (see §13):

- **Parameterized notations**: `\KL{p}{q}` and `D_{\mathrm{KL}}(p \| q)` as two
  spellings of one notation, both directions. The rule engine is a tree rewriter
  from the start so these arrive as table rows, not special cases.
- **Lints** on notation: an undeclared symbol, a symbol with two meanings, an
  engine-specific no-op such as `\mathbf{\alpha}` under pdflatex.
- **CAS export**: a symbol table a SymPy or Mathematica printer can read so
  generated equations use the paper's spellings.

Non-goals, permanently: general LaTeX formatting (whitespace, line breaking,
indentation), which `latexindent`, `tex-fmt` or `badness` do; compiling;
anything outside math mode; full TeX macro expansion.

## 3. Concepts

- **Spelling**: the bytes an author typed for a symbol. `\mathbb{R}`, `\R`,
  `\symbb{R}` and `ℝ` are four spellings.
- **Atom**: the canonical identity of a symbol. Every spelling resolves to one
  atom. Two kinds:
  - `Named(codepoint)`: a symbol that is one Unicode character and has no style
    dimension: `∈` U+2208, `≤` U+2264, `∑` U+2211, `∞` U+221E, `α` U+03B1.
  - `Styled { base, style }`: a letter or digit under a math alphabet: `(R, bb)`,
    `(x, bf)`, `(α, bfit)`, `(N, cal)`, `(1, bb)`. `base` is an ASCII letter or
    digit or a plain Greek codepoint. `style` is one of: `up it bf bfit bb cal scr
    bfcal bfscr frak bffrak sf sfit bfsf bfsfit tt`. A plain letter with no style
    command is not an atom; it is left alone in every profile.
  The Unicode codepoint is the pivot: unicode-math's table maps every codepoint to
  a command name and a math class, and the Mathematical Alphanumeric Symbols block
  (with its Letterlike Symbols fill-ins) gives most `Styled` atoms a codepoint.
  Some styles have none for some bases (upright Latin, `(d, up)`), and those
  atoms only ever print as a wrapper command.
- **Profile**: the policy for printing atoms. Which class of atom prints as
  Unicode and which as a command; which command spells each style; per-atom
  overrides; whether user macros are inlined. A project may hold several
  profiles; a run picks one.
- **Object** (deferred layer): a mathematical thing the paper talks about,
  declared with a name and the atom that stands for it (`sigma_fields = "ℱ"`).
  Renames and the future lints operate here. Version zero ships `rename` without
  requiring declarations.

## 4. Architecture

A library crate `mathfmt-core` and a thin CLI crate `mathfmt`. The library's
pipeline, per file:

```
bytes ──scan──▶ regions ──lex/parse──▶ math trees ──expand──▶ ──resolve──▶ atoms
                                                                        │
          edits ◀──print (profile)◀────────────────────────────────────┘
bytes ──apply edits──▶ bytes'
```

1. **Scan** finds math regions in a document and user macro definitions, skipping
   comments and verbatim. Everything else is opaque bytes.
2. **Lex and parse** each math region into a lossless tree: every byte of the
   region is in exactly one token; tokens are grouped into commands with
   arguments, brace groups, and text arguments (which recursively hold
   documents, since `\text{…}` may contain `$…$`).
3. **Expand** user macro calls, bounded, to see what atoms they stand for.
4. **Resolve** tokens and styled groups to atoms.
5. **Print**: for each atom, the profile's spelling. Where it differs from the
   source bytes, emit an edit `(byte range, replacement)`.
6. **Apply** the edits to the original bytes. Nothing outside an edit changes.

Outputs are the rewritten bytes, a list of diagnostics, and (on request) the
intermediate stages as JSON lines.

### Why not rowan

Rowan (rust-analyzer's lossless tree) was the plan when an LSP was imagined. The
tool is a batch filter; it never edits incrementally and never shares subtrees.
A token vector with byte spans plus a small tree of indices is lossless by
construction and a quarter of the code. If an LSP ever wants incremental
reparsing, the parser's interface (region in, tree out) does not change.

## 5. The scanner

Input is one file as a UTF-8 string. The scanner is a recursive descent over
two modes.

**Text mode** reacts only to:

| sees | does |
|---|---|
| `%` | comment to end of line, skipped |
| `\` + letters | a control word; `\` + one non-letter is a control symbol (so `\\(` is never `\(`; `\%` and `\$` are not comment or math) |
| `$` / `$$` | inline / display math until the matching unescaped `$` / `$$` |
| `\(` / `\[` | inline / display math until `\)` / `\]` |
| `\begin{E}`, E in `math_environments` | display math until `\end{E}` |
| `\begin{E}`, E in `verbatim_environments` | skipped until `\end{E}` |
| `\verb` / `\verb*` + delimiter | skipped until the delimiter |
| `\ensuremath{` | math until the matching brace |
| `\url{…}`, `\href{…}`, `\path{…}`, `\lstinline` | first argument skipped (URLs may hold `$`) |
| `\newcommand`, `\renewcommand`, `\providecommand` (and `*`), `\DeclareMathOperator` (and `*`), `\def`, `\NewDocumentCommand`, `\RenewDocumentCommand`, `\DeclareRobustCommand` | a definition: recorded (§7), and its body is skipped, never rewritten |

Default `math_environments`: `equation equation* align align* alignat alignat*
gather gather* multline multline* flalign flalign* eqnarray eqnarray*
displaymath math`. Default `verbatim_environments`: `verbatim verbatim*
lstlisting minted comment Verbatim`. Both extend from config.

**Math mode** lexes tokens (§6) until its terminator. Commands in
`text_commands` take a text argument: `\text \textrm \textit \textbf \textsf
\texttt \textnormal \mbox \hbox \intertext \shortintertext \operatorname
\operatorname* \label \tag \tag* \ref \eqref \cite \textcolor(1st arg) \color`.
A text argument is a nested document scanned in text mode, so `$a \text{ if
$b$ } c$` nests correctly. The terminator of a math region is found by the
parser, not by a string search, for the same reason.

Line and column for diagnostics come from byte offsets at the end.

## 6. Math tokens and tree

Tokens, each with a byte span into the file:

- `ControlWord(name)`: `\` + ASCII letters. A following `*` is not part of it.
  Unicode letters never extend a control word (matches pdflatex; a LuaTeX
  document relying on `\alphaβ` being one name is out of scope).
- `ControlSymbol(char)`: `\` + one non-letter, including `\\`, `\,`, `\{`, `\|`.
- `Open` `{`, `Close` `}`, `Sup` `^`, `Sub` `_`, `Align` `&`, `Param` `#n`.
- `Char(char)`: one Unicode scalar. Combining marks attach to the previous char.
- `Trivia`: whitespace and `%` comments.
- `Dollar`, `DoubleDollar`, and the `\)` `\]` terminators as `ControlSymbol`.

The tree: `Group{open, children, close}`, `Command{name, args}` where each arg
is `Brace(Group)`, `Bracket(span)`, `Token(single)` or `Text(nested document)`,
and `Leaf(token)`. Argument counts come from a built-in signature table (the
LaTeX kernel, amsmath, amssymb, bm, unicode-math's `\sym…` family) and from
user definitions. An unknown command has zero arguments. The tree is printed
back byte for byte when nothing is rewritten, by construction.

Style commands and their styles:

| style | commands read as it |
|---|---|
| up | `\mathrm \symup \textup`¹ |
| it | `\mathit \symit` |
| bf | `\mathbf \symbf \symbfup \mathbfup` |
| bfit | `\boldsymbol \bm \pmb \symbfit \mathbfit` |
| bb | `\mathbb \symbb \mathbbm \mathds` |
| cal | `\mathcal \symcal` |
| scr | `\mathscr \symscr` |
| bfcal / bfscr | `\mathbfcal \symbfcal` / `\mathbfscr \symbfscr` |
| frak / bffrak | `\mathfrak \symfrak` / `\mathbffrak \symbffrak` |
| sf sfit bfsf bfsfit | `\mathsf \symsf`, `\mathsfit \symsfit`, `\mathbfsf \symbfsf`, `\mathbfsfit \symbfsfit` |
| tt | `\mathtt \symtt` |

¹ `\textup` is a text command in reality; listed to show the table is data and
can be edited. It is not in the default table.

`cal` and `scr` share Unicode codepoints. Reading a script-alphabet codepoint
gives `cal` unless the profile says `script_reads_as = "scr"`. Printing either
to Unicode gives the same character. This is the one place the pivot is lossy,
and it is documented in the README.

## 7. Macro expansion

The expander recognizes:

- `\newcommand{\name}[n][default]{body}` and `\renewcommand`, `\providecommand`,
  starred forms, with or without braces around `\name`.
- `\DeclareMathOperator{\name}{text}`: `\name` becomes an operator word,
  recorded so it is never mistaken for a symbol; it is not inlined.
- `\def\name#1#2{body}` with undelimited parameters only.
- `\NewDocumentCommand\name{m m}{body}` with `m` and `o` argument specs only.
- `\DeclareRobustCommand` as `\newcommand`.
- `\ensuremath{…}` in a body is unwrapped.

Expansion substitutes `#n` with the argument's tokens and re-parses the result,
to a depth of 16. A macro that reaches the depth, has a delimited parameter, or
uses `\if…`, `\csname`, `\expandafter`, `\def` inside its body, or any other
primitive the expander does not model, is **opaque**: its calls are left
untouched and one diagnostic names it.

Expansion exists for recognition. A call is *inlined* in the output only when
its expansion resolves entirely to atoms and trivia and the profile has
`inline_macros = true`. `\KL{p}{q}` therefore survives version zero untouched.
The reverse, folding atoms into a macro call, is a per-atom spelling in the
profile (`spellings."ℝ" = "\\R"`).

Definition sources, in order: files named by `macros = [...]` in the config,
files named by `--macros`, then the file being converted (its own preamble).
Later definitions win, as in TeX.

## 8. Resolution

Walking a math tree:

1. A `Char` whose codepoint is in the symbol table resolves to `Named(cp)`,
   or to `Styled` by the alphabet table (𝐱 → `(x, bf)`). ASCII chars never
   resolve; they are literal. U+2212 (minus) is the one exception: it resolves
   to the ASCII `-` spelling in every profile.
2. A `ControlWord` in the name table (unicode-math primary names and the
   LaTeX/amsmath/amssymb aliases) resolves to its atom: `\in` → `Named(2208)`,
   `\alpha` → `Named(3B1)`, `\varphi` → `Named(3C6)`, `\phi` → `Named(3D5)`,
   `\emptyset` and `\varnothing` → `Named(2205)`.
3. A style command whose argument consists only of atoms-or-bare-letters-or-
   digits-or-Greek (and trivia) resolves to a run of `Styled` atoms, one per
   base. `\mathbf{xy}` → `(x,bf)(y,bf)`. `\mathbf{\alpha}` → `(α, bf)`.
4. A style command whose argument holds anything else (`\mathbf{\sigma_\theta}`,
   `\mathrm{Var}` is fine since letters are bases, but `\mathbf{x+y}` is not) is
   left whole: nothing inside it is rewritten. Documented, and the first lint
   candidate.
5. A user macro call expands (§7); if the expansion is atoms-and-trivia, the
   call resolves to those atoms.
6. Everything else is not an atom and is never touched.

## 9. Profiles and printing

A profile is:

```toml
[profiles.unicode]
symbols = "unicode"          # Named atoms: "unicode" | "latex"
styled  = "unicode"          # Styled atoms: "unicode" | "latex"
greek   = "unicode"          # plain Greek letters: "unicode" | "latex"
inline_macros = true         # replace \R by its atom's spelling
script_reads_as = "cal"      # or "scr"
[profiles.unicode.spellings]  # per-atom overrides, canonical -> spelling
"∅" = "∅"

[profiles.ascii]
symbols = "latex"
styled  = "latex"
greek   = "latex"
inline_macros = true
[profiles.ascii.style_commands]
up = "\\mathrm"
it = "\\mathit"
bf = "\\mathbf"
bf_greek = "\\boldsymbol"    # pdflatex's \mathbf ignores Greek
bfit = "\\boldsymbol"
bb = "\\mathbb"
bb_digit = "\\mathbbm"       # \mathbb{1} is not a thing in amsfonts
cal = "\\mathcal"
scr = "\\mathscr"
frak = "\\mathfrak"
sf = "\\mathsf"
tt = "\\mathtt"
[profiles.ascii.spellings]
"→" = "\\to"
"ℝ" = "\\R"                  # a collaborator who wants macros
```

Printing rules:

- `Named` under `unicode`: the character. Under `latex`: the table's preferred
  LaTeX name; when the codepoint has none that pdflatex+amsmath+amssymb define,
  the atom is left as written and a diagnostic `no-latex-spelling` names it.
- `Styled` under `unicode`: the alphabet codepoint when one exists, else the
  wrapper command from `style_commands`. Under `latex`: the wrapper command.
  Adjacent `Styled` atoms with the same style and no trivia between them share
  one wrapper: `𝐱𝐲` → `\mathbf{xy}`.
- Greek under `latex`: `\alpha` etc. Uppercase Greek that LaTeX has no name
  for (Α, Β, …) is left and diagnosed.
- `spellings` override everything for that atom.
- A spelling that ends in a control word and is followed in the output by a
  letter gets one space inserted, so `αx` → `\alpha x`. Math mode ignores the
  space, so inserting one is always safe. Existing trivia is kept as it was.
- Nothing is emitted for an atom whose spelling already equals its source bytes.

Determinism: output depends only on input bytes, config and the data tables.
Idempotence: running the output through the same profile changes nothing; the
test suite asserts this for every fixture.

## 10. Configuration

`mathfmt.toml`, found by walking up from each input file's directory (like
`rustfmt.toml`), or `--config PATH`. Stdin uses the current directory.

```toml
version = 1
profile = "unicode"                 # default when --profile is absent
macros = ["ddssm.sty", "main.tex"]  # definition sources, relative to this file
math_environments = ["IEEEeqnarray"]       # added to the defaults
verbatim_environments = ["tcblisting"]     # added to the defaults
text_commands = ["\\mynote"]               # added to the defaults

[profiles.<name>]  # as in §9; "unicode" and "ascii" exist with the defaults
                   # above even when the file does not mention them

[symbols]          # the object layer, optional in version zero
reals = { atom = "ℝ", means = "the real numbers" }
```

Unknown keys are errors (exit 2) so typos do not silently disable a rule.
`--profile NAME` must name a profile that exists.

## 11. Command line

```
mathfmt [OPTIONS] [FILE...]
    no FILE: read stdin, write stdout
    -p, --profile NAME        profile from the config (default: config's `profile`)
    -c, --config PATH         config file (default: nearest mathfmt.toml)
        --macros FILE...      extra definition sources
    -w, --write               rewrite FILEs in place (atomic: temp file + rename)
        --check               exit 1 if any file would change; print their names
        --diff                print a unified diff per changed file
        --emit KIND           regions | tokens | atoms | symbols, as JSON lines
        --format text|json    diagnostics format (default text)
        --strict              warnings make the exit status 1
    -j, --jobs N              parallel files (default: available cores)
    -q, --quiet               no diagnostics on stderr

mathfmt rename --from SPELLING --to SPELLING [same options] [FILE...]
    both spellings must resolve to exactly one atom

mathfmt table [--profile NAME]
    dump the effective symbol table as TSV: codepoint, atom, unicode, latex, aliases
```

Exit status: 0 clean; 1 `--check` found changes or `--strict` found warnings;
2 usage, config or I/O error. Diagnostics: `file:line:col: level[code]:
message` on stderr, or one JSON object per line with `--format json`. Silent on
success.

`--emit symbols` prints one object per distinct atom per file: `{"file", "atom",
"unicode", "name", "count", "spellings": {"\\mathbb{R}": 40, "\\R": 1}}`. This is
the notation inventory, meant for `jq`.

Files are processed in parallel; definitions are loaded once and shared.

## 12. Data files and their generator

`crates/mathfmt-core/data/`:

- `unicode-math.tsv`: generated by `tools/gen-table.py` from
  `kpsewhich unicode-math-table.tex` (2,448 rows in TeX Live 2025). Columns:
  codepoint, unicode-math name, math class, description. Checked in; the
  generator is run by hand when TeX Live updates and the diff is reviewed.
- `alphabets.tsv`: generated by the same script by parsing names such as
  `\mbfalpha` into (style prefix, base). Columns: codepoint, style, base.
- `latex.tsv`: hand-curated. Columns: codepoint, preferred LaTeX spelling for
  the `latex` profile, accepted input aliases. Only names defined by the LaTeX
  kernel, amsmath, amssymb and bm appear in the second column. This is the file
  a user edits to add a symbol.
- `signatures.tsv`: hand-curated command argument counts and text-argument
  flags.

The binary embeds these with `include_str!` and parses them at startup
(microseconds). `mathfmt table` prints the merged result so a user can see
exactly what the tool believes.

## 13. Roadmap after version zero

1. **Parameterized notations** (the approved option 2). A notation row has a
   pattern with holes: `KL = { macro = "\\KL{#1}{#2}", pattern = "D_{\\mathrm{KL}}(#1 \\| #2)" }`.
   Matching is tree matching with hole binding on the math tree; printing is
   substitution. The resolver gains a pass before atoms. The expander already
   produces the tree the matcher needs.
2. **Object layer and lints**: `[symbols]` declarations; lint `undeclared-atom`
   (opt-in), `two-spellings-one-atom` (always, as info), `mathbf-greek-pdflatex`
   when a profile names an engine.
3. **Per-user overlay**: `~/.config/mathfmt/config.toml` layered under the
   project's, for the collaborator who wants macros in their working copy.
4. **CAS export**: `mathfmt table --format sympy` prints a `symbol_names` dict.
5. **Editor integration** is free through the filter interface; a document is
   written, not code.

## 14. Testing

- `tests/fixtures/<case>/`: `input.tex`, `mathfmt.toml` (optional; the default
  config otherwise), `expected.tex`, and optionally `expected.stderr` and
  `args` (one CLI argument per line). The harness in
  `tests/golden.rs` runs each case through the library,
  compares bytes, then asserts idempotence (the expected output through the
  same profile is unchanged).
- Round trip: fixtures tagged `roundtrip` are run `unicode → ascii → unicode`
  and must return to the start.
- Unit tests in each module for the scanner's edge cases (`\$`, `\\(`, `%` in
  display math, `$` inside `\text{}` inside `$…$`, `\verb|$|`), the expander's
  refusals, and the printer's spacing rule.
- A smoke test target, not in CI: `--check --diff` over the ICML paper
  repository, read-only.

## 15. Repository layout

```
mathfmt/
  Cargo.toml                 workspace
  src/main.rs                the CLI; the root is the CLI package so pre-commit's
                             `cargo install --path .` works
  crates/mathfmt-core/       the library: scan, lex, parse, macros, resolve, profile, print, config, diag
  tests/golden.rs, cli.rs    the integration tests
  crates/mathfmt-core/data/  the tables (§12)
  tools/gen-table.py         the generator (§12)
  tests/fixtures/            golden cases (§14)
  tests/dev.sh               run a command in the dev shell (seats have no nix on PATH)
  flake.nix                  dev shell, package, checks; vendored crates for offline cargo
  .pre-commit-hooks.yaml     so other repositories can use this one as a hook
  .phalanix/                 the rig: lead + task/review/question templates
  docs/superpowers/specs/    this file
  docs/superpowers/plans/    the plans, one per wave
  CLAUDE.md                  how to work here
```

## 16. Packaging and platforms

- Rust, edition 2021, MSRV 1.80. No unsafe. Dependencies: clap, rayon,
  similar, anyhow, serde, serde_json, toml.
- `flake.nix`: `packages.default` via `rustPlatform.buildRustPackage` with
  `cargoLock`; `devShells.default` with the toolchain and `CARGO_VENDOR_DIR`
  pointing at `importCargoLock`'s output so `tests/dev.sh cargo test` works with
  no network (a phalanix seat's proxy does not allow crates.io; the nix daemon's
  fetches are unfiltered); `checks` run `cargo test` and `cargo clippy`.
- Linux and macOS through the flake and `cargo install`; Windows through
  `cargo install` and, later, release binaries (`cargo-dist`).
- pre-commit: `.pre-commit-hooks.yaml` with id `mathfmt` (`language: rust`,
  `args: [--write]`, `files: \.(tex|sty)$`).

## 17. Decisions

- Canonical identity is the Unicode codepoint, extended with a style dimension
  for letters, because unicode-math already maintains the table and because
  Unicode is what the readable profile emits.
- No rowan (§4).
- Style semantics are literal (`\mathbf` is `bf`, bold upright) and engine
  rendering quirks are lint material, except `bf_greek`, which defaults to
  `\boldsymbol` because the `latex` profile exists for pdflatex and
  `\mathbf{\alpha}` renders nothing bold there. This collapses 𝛂 and 𝜶 to the
  same ASCII; the README says so and the key is configurable.
- `cal` and `scr` share codepoints; `cal` wins on read.
- Unknown config keys are errors.
- A style group that is not purely atoms is left whole (§8.4) rather than
  partially rewritten.
- The tool never writes a file unless `--write` is given, and never recurses
  into directories: the caller lists the files.

## Amendments

### 2026-11-05, from building version zero

- **§8.3, upright words.** A style group under `up` whose bases include an
  ASCII letter or digit (`\mathrm{Var}`, `\mathrm{d}`, `\mathrm{tr}`) is an
  operator word, not a symbol: it is never an atom, never rewritten, never
  counted, and a zero-argument macro expanding to one (`\tr`) is not inlined.
  Upright Greek (`\mathrm{\pi}`) stays a `Styled` atom, since upright Greek
  carries meaning and the alphabet rule applies. Found on the ICML paper, where
  `\tr` would otherwise have been inlined to `\mathrm{tr}` and the inventory
  listed `data`, `diag`, `trans` letter by letter.
- **§9, script operands.** A replacement that starts with a non-ASCII character
  and is the operand of `^` or `_` (spaces allowed in between) is wrapped in
  braces: `x^\alpha` → `x^{α}`. pdflatex with `inputenc` reads a multi-byte
  character one byte at a time after `^`; the braces cost nothing anywhere and
  the output is still idempotent (the second run sees `{α}`).
- **§9, U+2212.** A curated spelling that is plain ASCII (`-` for U+2212) is the
  one spelling under every profile; the `unicode` form never emits `−`.
- **§9, curated spellings for styled atoms.** Under `styled = "latex"`, a
  `Styled` atom with a row in `latex.tsv` prints that row (`ℑ` → `\Im`, `ℜ` →
  `\Re`) rather than its wrapper. Override per profile with `spellings`.
- **§9, wrapper templates.** A `style_commands` value may contain `#1`, for
  styles pdflatex spells by nesting: `bfcal = "\\boldsymbol{\\mathcal{#1}}"`.
  `sfit` and `bfsfit` have no default and are reported as `no-latex-spelling`.
- **§11, flags.** `--macros FILE` is repeatable, one file per flag (a
  multi-value flag swallowed the positional file list). `-v`/`--verbose` shows
  informational diagnostics (`style-group-left-whole`, `opaque-macro`,
  `definition-not-understood`), which are hidden by default. `--emit regions`
  and `--emit tokens` cover top-level regions only, not those nested in text
  arguments. `mathfmt table` prints codepoint, atom key, character, curated
  LaTeX spelling, the named profile's spelling, unicode-math name and aliases.
- **§4, the tree.** As anticipated, no rowan: tokens with byte spans and a
  tree of groups and commands, with the output produced as edits on the
  original bytes. Losslessness follows from never touching bytes outside an
  edit.
- **§7, inlining.** Expansion is textual (`#n` replaced by the argument's
  source, one level of braces stripped from a group argument as TeX does), then
  parsed as math. A call is inlined only when every non-trivia token of the
  expansion resolved to an atom; ASCII letters, punctuation and spacing
  commands in the body keep the call as written, while its arguments are still
  converted (`\KL{\alpha}{β}` → `\KL{α}{β}`).
- **§6, `cal`/`scr` on read.** The profile key is `script_reads_as`; the
  default `cal`.
- **§14, fixtures.** Thirteen cases shipped; the smoke test over the ICML
  paper (eight files, 677 occurrences, 48 distinct atoms) converts in about
  25 ms with no warnings, and the unicode → ascii → unicode round trip returns
  every file byte for byte.

### 2026-11-07, from the first adversarial review

Fifteen findings; all accepted.

- **§15, layout.** The root manifest is the CLI package and the library is a
  workspace member. pre-commit's `language: rust` runs
  `cargo install --path .` at the hook repository's root, which a virtual
  workspace refuses.
- **§16, dev shell.** The vendored-source replacement is written to a
  per-checkout `.cargo-home/config.toml` by the shell hook, not passed as
  `--config` flags: cargo does not forward those to external subcommands, so
  `cargo clippy` in a seat looked for crates.io. Every cargo entry point,
  rust-analyzer included, now sees the same offline configuration.
- **§11, flags.** `--strict` counts warnings whether or not `--quiet` hides
  them. `--write` requires at least one FILE (a usage error, exit 2, instead
  of a panic on stdin) and conflicts with `--diff`. `--write` follows a
  symlink and replaces its target, so the link stays a link. `--emit` prints
  in argument order when inputs span several config files.
- **§7, `\ensuremath`.** Signatures gain a `transparent` flag; `\ensuremath`
  carries it, so a body of `\ensuremath{\mathbb{R}}` inlines as the spec
  always said.
- **§5, verbatim arguments.** The first argument of `\url`, `\href`, `\path`,
  `\nolinkurl` and `\lstinline` is read with braces counted and `%` and `\`
  literal (a percent-encoded URL used to comment out the rest of the line),
  and a delimiter form (`\lstinline|…|`, `\url|…|`) is skipped like `\verb`,
  with any `[options]` before it.
- **§4, the macro table.** The parser borrows the definitions in force and
  collects a file's own in a small local table merged on return, so an
  expansion no longer copies the whole table.
- **§14, the harness.** `args` files support `--profile`, `--macros` and
  `rename --from --to`, as CLAUDE.md now says; a flag with no value panics
  with its name; the round trip's starting profile falls back to the case's
  config.
- The README's staged-files example read stdin when nothing was staged; the
  example names files and the stdin fallback is stated next to the exit codes.
