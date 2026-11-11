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
  `cargo install`; every platform through the Python wheel (amendment of
  2026-11-10, "the Python distribution").
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

### 2026-11-08, style distribution

- **§8.3 and §8.4, style groups.** A style command's alphabet reaches through
  scripts and braces, as it does in TeX: `\mathbf{\sigma_\theta}` is bold `σ`
  and bold `θ`, `\mathbb{R^n}` is `ℝ^{𝕟}`, `\bm{\mu_k}` is `𝝁_{𝒌}`. The
  resolver applies the style to every base inside the group and keeps the
  scripts and braces as structure (an occurrence now carries `parts`, the atoms
  interleaved with that structure; a run of bases is flat as before).
  Whitespace inside the group is dropped, as math ignores it. A group that
  holds anything else (an operator, punctuation, another command, a comment)
  is still left whole, with the same informational diagnostic; those are the
  cases where TeX's result is not a sequence of styled letters. The upright
  rule of 2026-11-05 is unchanged: `\mathrm{x_1}` is an operator word.
- **§9, printing a structured group.** Under `styled = "latex"`, when every
  atom in the structure wants the same wrapper, one wrapper encloses the
  structure as the source had it (`\bm{\mu_k}` stays `\bm{\mu_k}`;
  `\mathbf{\sigma_\theta}` becomes `\boldsymbol{\sigma_\theta}` by the
  `bf_greek` key). Otherwise each atom is spelled on its own inside the
  structure: `\mathbf{\sigma_x}` → `\boldsymbol{\sigma}_{\mathbf{x}}`,
  `\mathfrak{I_x}` → `\Im_{\mathfrak{x}}`. Under `unicode`, each base becomes
  its character: `𝛔_{𝛉}`. Structured occurrences never merge with their
  neighbours.
- **§9, script braces for wrappers.** A replacement that is a command with an
  argument and stands as the operand of `^` or `_` is wrapped in braces, like
  a multi-byte character: `x_\boldsymbol{\theta}` is a fatal "Missing {
  inserted" in pdflatex, so `𝐱_𝐲` → `\mathbf{x}_{\mathbf{y}}`. A bare control
  word (`x^\alpha`) is left bare as before.
- **§14, the round trip and the paper.** Distribution converts the two groups
  the smoke test used to leave whole, and exposes the `bf_greek` collapse of
  §17 on real text: `\mathbf{\sigma_\theta}` → `𝛔_{𝛉}` → ascii
  `\boldsymbol{\sigma}_{\boldsymbol{\theta}}` → `𝝈_{𝜽}`, bold italic where
  the source had bold upright. The round trip over the paper now returns seven
  files of eight; a paper that wants it closed under pdflatex's reading of
  bold Greek sets `bf_greek = "\\mathbf"` in its profile. The compile-and-
  compare check of §13 is what decides such questions from here on.

### 2026-11-08, the compile-and-compare check

- **§11, `--verify`.** A mode of the conversion, beside `--check` and
  `--diff`: the original and the converted source are compiled with the same
  engine (`--engine`, default `pdflatex`), both PDFs are rasterized with
  `pdftoppm` (`--dpi`, default 100) and compared pixel for pixel; a verdict
  per document is printed (`verified`, `unchanged`, `differs` with every
  differing page and its box, `broken` with the log's first error) and a
  difference makes the exit status 1. `pdftotext` supplies a hint, the first
  differing line of text, when it is available; it cannot tell a bold letter
  from a plain one, which is why the verdict rests on pixels. `rename` takes
  the flag too, so a rename can be shown to change exactly what it should.
  `--keep DIR` keeps the two build trees. It works for `--verify` to need the
  engine on PATH; mathfmt itself stays a pure filter.
- **§11, documents and snippets.** A FILE holding `\documentclass` outside a
  comment is a document: it is compiled as it is, from a copy of all the FILEs
  at their relative paths, with the document's own directory on `TEXINPUTS`
  so unlisted style files, figures and inputs are shared; the original is
  compiled the same way from its own copy, so both sides see the same files.
  A FILE without one is a snippet, wrapped in a preamble: `--preamble FILE`,
  or the built-in one for the engine. Lines of a snippet that hold a
  preamble-only command (`\usepackage`, `\DeclareMathOperator`, …) move into
  the preamble on both sides. The definition files the engine was given
  (`macros` in the config, `--macros`) are loaded by the wrapper.
- **The built-in pdflatex preamble** loads amsmath, amssymb, bm, bbm,
  mathrsfs, mathtools, url, listings and unicode-math-input, and declares
  every styled letter or digit the snippet uses, before or after conversion,
  to mean what the `ascii` profile spells it as (`\umiDeclareMathChar{𝟙}
  {\mathbbm{1}}`, `\umiDeclareMathChar{𝛂}{\boldsymbol{\alpha}}`). pdflatex
  has no bold Greek under `\mathbf` and no blackboard digits under `\mathbb`,
  and unicode-math-input follows pdflatex there, so without the declarations
  the Unicode side would render as pdflatex's limitations rather than as the
  table's meaning. Symbols (`∈`, `≤`, `∑`) are left to unicode-math-input's
  own table, which is the independent check of `latex.tsv`. A document is
  never given declarations: it is verified under its author's preamble, and
  that is the point.
- **§14, the verify test.** `tests/verify.rs` runs `--verify` over every
  fixture's `input.tex` with the case's `args`, expecting `verified` or
  `unchanged` unless `expected.verify` says `differs: reason` (a rename; a
  spelling that repairs or changes the typeset result on purpose) or
  `skip: reason`. It is skipped without pdflatex and pdftoppm on PATH, and
  required under `MATHFMT_VERIFY_REQUIRED`, which the flake's `verify` check
  sets with a TeX Live that has the preamble's packages.
- **What the check found on its first run.** Six of the seventeen fixtures.
  Three were fixture faults (folded macros no document defined; a
  `\DeclareMathOperator` in the body; `∆`, which nothing in pdflatex
  defines). Three were real: `\mathbb{1}` is the glyph ⊮ in amsfonts, so the
  `bb_digit` spelling `\mathbbm{1}` repairs it and changes the page;
  `\mathfrak{I}` and `\Im` share U+2111 but are different glyphs in pdflatex
  (eufm against cmsy), a third collapse beside `cal`/`scr` and `bf_greek`;
  and `\mathbf{\alpha\beta}` is plain under pdflatex and bold after
  conversion, the `bf_greek` collapse again. Those three live in fixtures
  marked `differs`, with the reason in the file. On the paper, in document
  mode with `unicode-math-input` added to its preamble, every page renders
  the same except the indicator `𝟙`, which unicode-math-input reads as
  `\mathbb{1}` where the paper wrote `\mathbbm{1}`; the paper's preamble
  would declare it.
- **§13, engines.** Under `lualatex` with `unicode-math`, `\mathbf{x}` is the
  text font's bold and `\boldsymbol{\alpha}` is not bold, so the default
  `ascii` profile is not faithful there; the `\sym…` commands are. A profile
  for unicode-math is configuration today (`style_commands`), a built-in
  preset later.

### 2026-11-08, parameterized notations (§13.1 shipped)

- **§3, notations.** A notation is a named symbol with parameters, and its
  forms are its spellings, as an atom's spellings are an atom's. The config
  declares them:

  ```toml
  [notations.KL]
  macro    = "\\KL{#1}{#2}"
  expanded = "D_{\\mathrm{KL}}(#1 \\| #2)"
  spaced   = "D_{\\mathrm{KL}}(#1 \\,\\|\\, #2)"

  [profiles.ascii.notations]
  KL = "macro"
  [profiles.unicode.notations]
  KL = "expanded"
  ```

  Every form is recognized on input. A profile's `notations` names the form
  it writes; a profile that names none leaves the notation as written but
  still sees it (the inventory counts it, `--emit atoms` lists it with
  `rewritten: false`). Validation: holes are `#1`…`#n` without gaps, two
  holes never stand next to each other (the split would be ambiguous), every
  form of a notation has the same holes, and a profile's choice names a form
  that exists.
- **§8, matching.** Forms are parsed as math with the document's own
  definitions, so `\KL{#1}{#2}` has the shape a `\KL{p}{q}` call has whether
  or not `\KL` is defined. Matching is tree matching modulo atoms: a leaf
  matches a leaf that stands for the same atom (`\|` and `‖`, `\le` and
  `≤`), else the same text; a command matches a command of the same name,
  star and arity with matching arguments; a group matches a group with
  matching children, and a group of one matches that one node. A hole binds
  the shortest run of nodes that lets the rest of the pattern match; a hole
  that fills a group (`{#1}`) binds the group's contents; a hole at the end
  of a pattern binds the rest of its group, or one node at the top level. A
  repeated hole must bind the same text. At a match's first node a pattern
  node that is not a group never matches a document group by looking inside
  it: the match would cover braces the rewrite does not reproduce
  (`\underbrace{\KL{p}{q}}` lost its braces on the paper before `--verify`
  caught it); the walker descends into the group and matches there. Forms
  are tried most specific first
  (most tokens), then notations in config order; the first match wins. The
  resolver tries the notations at every position of every region before the
  atom rules; the literal parts of a rewritten match are the form's and are
  not converted, the holes are walked (atoms, and nested notations). Inside a
  macro expansion no notation is matched: its spans are not the document's.
- **§9, rewriting.** The profile's form is filled with the holes' converted
  text (inner matches first, so an outer hole already holds its nested
  rewrites), a hole after `^` or `_` is braced unless it is one ASCII token,
  the spacing rule applies at every seam, and the result is run through the
  profile's atom spellings once more, so a form's own literals come out in
  the profile's spelling (`\|` → `‖` under `unicode`) and one form serves
  every profile. The edits the match covers are consumed by it. A rewrite is
  idempotent because the output is itself one of the recognized forms.
- **§11.** `--emit symbols` adds one line per notation (`notation`, `count`,
  `forms`); `--emit atoms` adds one per match (`kind: "notation"`, `form`,
  `holes`, `rewritten`).
- **§14.** Fixtures `notation-to-macro`, `notation-to-expanded` and
  `notation-spaced-form`; the last is marked `differs` because unifying
  `D_{\mathrm{KL}}(p \,\|\, q)` with `\KL{p}{q}` drops the thin spaces, which
  is what a notation with several forms is for.
- **Not yet.** Holes inside optional arguments or `\text{}` (those compare as
  text); a form whose macro has an optional argument; notation renames; the
  object layer (`[symbols]`) over notations.

### 2026-11-09, project mode

- **§3, project.** A project is a root document (the file holding
  `\documentclass`), the files its body reads with `\input`, `\include`,
  `\subfile`, `\InputIfFileExists`, `\import` and `\subimport`, and a
  preamble that loads packages, some of them local files beside the root.
  `project.rs` reads that much with a token walk and no TeX: the files in
  reading order, each once; the packages, through local `.sty` and `.cls`
  files and files the preamble inputs, which are definition sources; the
  class; the engine a `% !TEX program = …` comment names. It does not
  evaluate conditionals, skips a file name that holds a macro or a
  parameter, stops a file at `\endinput` and the root at `\end{document}`,
  and knows a distributed package by its name and a short table of what it
  loads (`mathtools` loads `amsmath`). A name the body inputs that no file
  beside the root answers is recorded as missing and otherwise ignored.
- **§7, definition sources.** With a root, the order is: the local packages
  and classes and the preamble's inputs, in load order; the root itself; the
  config's `macros`; `--macros`; the file being converted. So a section
  handed to the tool alone knows the paper's macros.
- **§10, config.** Three keys at the top level: `root` (the root document,
  relative to the config), `engine` (the program `--verify` compiles with)
  and `unicode_input` (`unicode-math-input`, `kernel` or `unicode-math`).
  One in a profile: `style_preset` (`latex` or `unicode-math`), the family of
  commands its `style_commands` start from.
- **§11, the command line.** `--root FILE` names the root for the FILEs given
  (default: the config's `root`). `--project` takes the files from the root
  instead: each FILE is a root, or with no FILE `--root`, or the config's
  `root`; a file two roots share is converted once, with the first. `--write`
  accepts `--project` in place of FILEs. `mathfmt project [ROOT]` prints what
  was read, one `key<TAB>value` per line. Inputs are grouped by config and
  root, one engine each.
- **§9, engines.** A profile that does not set `style_preset` gets
  `unicode-math` when the project's preamble loads that package, `latex`
  otherwise. The `unicode-math` preset is `\symbf`, `\symbfit`, `\symbb`,
  `\symcal`, `\symscr`, `\symfrak`, `\symsf`, `\symsfit`, `\symbfsf`,
  `\symbfsfit`, `\symtt`, `\symup`, `\symit` and the bold script and fraktur
  forms. `bf` is `\symbf` and not `\symbfup` because of what the package's
  default options do, measured with lualatex and `pdftotext`: the literals
  `𝐱𝒙𝛔𝝈𝚺𝜮` come out as `𝐱𝐱𝝈𝝈𝚺𝚺`, exactly as `\symbf` of each base does,
  and `𝖠𝘈𝗔𝘼` as `𝖠𝖠𝗔𝗔`, as `\symsf` and `\symbfsf` do. The shape of a bold
  or sans letter is the document's there, not the character's. This is a
  fourth collapse: a letter whose shape contradicts the house style changes
  when it crosses between a character and a command, and `--verify` says so.
  `--engine` defaults to the config's `engine`, else the root's `% !TEX
  program`, else `lualatex` when the preamble loads `unicode-math` or
  `fontspec`, else `pdflatex`.
- **§11, `mathfmt declarations`.** Prints the preamble lines the notation
  needs: a `\usepackage` for each package a command of the profile's LaTeX
  spellings comes from and the preamble does not load (every such package
  when no root is known), then a meaning for the Unicode characters of every
  atom the sources use, before or after conversion: the character as the
  resolver reads it, spelled by the profile with every form forced to
  commands. The mechanism is `--with`, else `unicode_input`, else
  `unicode-math` when the preamble loads it or the engine is not pdflatex,
  else `unicode-math-input`. Under `unicode-math-input` only styled letters
  are declared (`\umiDeclareMathChar`), the package's own table serving the
  symbols; under `kernel` every character is
  (`\DeclareUnicodeCharacter{XXXX}{\ensuremath{…}}`); under `unicode-math`
  none. A character with no LaTeX spelling is named in a comment. The
  snippet preamble of `--verify` is now built from the same function, so a
  styled letter means what the run's profile spells it as, where it meant
  the `ascii` profile's spelling before.
- **§12, `packages.tsv`.** Generated by `tools/gen-packages.py`, which asks
  TeX where each command of a spelling in `latex.tsv` or `greek.tsv`, and
  each math-alphabet command of `signatures.tsv`, is defined: the kernel, or
  the first of amsmath, amsfonts, amssymb, bm, bbm, mathrsfs, dsfont (under
  pdflatex) and unicode-math (under lualatex) that defines it. 398 commands,
  none undefined: the hand-written `latex.tsv` passes its first independent
  check. Two unit tests fail when a command has no row.
- **The mechanisms, measured under pdflatex.** Fourteen contexts, three
  mechanisms. All three read `\left⟨`, `x_α`, `\sqrt α`, `\mathbf{Σ}` and
  `∑_{i}`. Only unicode-math-input reads `\big⟨` and an unbraced styled
  script `x_𝛂`. Only the kernel form with `\ensuremath` reads a character
  inside `\text{…}`. None reads a character as the unbraced argument of a
  macro: `\hat α` and `\frac α β` fail under all three, because the macro
  takes the first byte. That is why unicode-math-input is the default, and
  it is a defect in the printer, which writes `\hat α` for `\hat\alpha`
  today; the fix is its own change.
- **§11, `--verify`.** `--declare` puts what `mathfmt declarations` prints
  before the first `\begin{document}` outside a comment, on both sides, and
  the verdict says how many lines were added; a document is still compiled
  under its author's preamble without the flag. A `broken` verdict whose
  error is an unset Unicode character names `mathfmt declarations` and
  `--declare`. The engine is per document. In project mode a document is
  compiled with the files of its own project only. The output tree gets the
  subdirectories of the sources, without which `\include{sections/x}` could
  not write its `.aux` and the original itself failed to compile.
- **§14, tests.** `tests/projects/<case>/` holds a document tree with a
  `mathfmt.toml` naming its `root`, the converted files under `expected/` and
  `expected.declarations`; `tests/project.rs` converts every file the root
  pulls in, compares, converts again, and compares the declarations.
  `tests/verify.rs` compiles each case with `--project --verify --declare`
  under the engine its preamble asks for, and checks that the `paper` case
  without `--declare` is `broken` and says what to run. Two cases: `paper`
  (pdflatex, a local package, `\input` and `\include`) and `unicode-math`
  (lualatex, the `ascii` profile under the preset). The flake's `verify`
  check gains unicode-math, fontspec, lualatex-math and lm-math.
- **On the paper.** `mathfmt --project main.tex --verify` from the paper's
  directory, nothing copied or edited: seventeen files, `broken` (the
  preamble maps some Unicode by hand and not `𝐱`). With `--declare`: one
  page of fourteen differs, the `bf_greek` collapse on
  `\mathbf{\sigma_\theta}`. With `bf_greek = "\\mathbf"` in the profile:
  `verified: main.tex (14 pages identical)`, 28 declaration lines, eight
  seconds.
- **Not yet.** Reading unicode-math's options (`bold-style=literal`) to pick
  the explicit commands; a package found through kpathsea rather than beside
  the root; conditionals; a subfile with its own `\documentclass`; a check
  that a committed declarations file is current; declarations for the
  literals of a notation form no source uses yet.

### 2026-11-09, the second review

An independent review of the three waves of 2026-11-08 (style
distribution, `--verify`, notations) returned twenty-two findings, all
reproduced; `docs/REVIEW-2026-11-09.md` lists them with what was done. The
behaviour that changed:

- **§9, a style group is one unit.** A structured style group
  (`\mathbf{x^2}`) spelled piece by piece is wrapped in braces when a script
  or a prime follows it, unless it is already one group or one command over
  one group: `\mathbf{x^2}^3` is `{𝐱^{𝟐}}^3`, where `𝐱^{𝟐}^3` was a double
  superscript. Text is one group only when the brace that opens it closes
  it at the end: `{𝐱}_{𝟏}` as a script operand is braced whole.
- **§9, an unbraced argument stays one argument.** The resolver marks an
  occurrence that is the whole of an unbraced math argument of a command
  (`\hat\alpha`, each of `\frac\alpha\beta`). Its replacement is braced when
  it is not ASCII or holds a group, as a script operand is: `\hat{α}`,
  `\frac{α}{β}`, `\hat{\mathbf{x}}`. A macro takes one token, which under
  pdflatex is one byte of the character and under any engine the bare
  `\mathbf` of a wrapper. Two such arguments never merge into one wrapper.
  Delimiter arguments (`\left⟨`, `\big⟨`) are left alone: `\left{⟨}` is not
  TeX.
- **§8, forms.** A form must hold something literal, balanced braces and no
  comment, and its first and last tokens must not be holes; the config
  refuses it otherwise. (A form of holes alone matched everything and
  overflowed the stack; a hole at the edge bound back to the start of the
  region.) Forms are tried most specific first across all notations, ties
  by notation name and then form name: the order no longer depends on which
  notation comes first, and `notations in config order` in the amendment of
  2026-11-08 is withdrawn, the config's map having no order to keep.
- **§8, holes.** A bare hole binds the shortest run that is balanced in its
  delimiters (parentheses, brackets, `\{ \}`, `\langle \rangle`, floors,
  ceilings, `\left \right`, kinds not told apart so `[0, 1)` is balanced)
  and lets the rest match; a run that closes a delimiter it did not open
  ends the search, which also bounds it (1600 terms: 20 s before, under a
  tenth of a second now). `D_{\mathrm{KL}}(p \| q(x))` binds `q(x)`.
- **§8, what is never a match.** A stretch holding a `%` comment or an
  unclosed group. (The first cut a rewritten form in two; the second let a
  match run to the end of the file and delete it.) A document group
  standing alone in a list is not its content; one that is a script operand
  or a command's argument is. This replaces the first-node rule of
  2026-11-08 and covers what it covered.
- **§8, styled literals.** A style command over one base (`\mathbb{E}`) in a
  form or in the document is the atom its character is, so `𝔼_{x ∼ p}[f]`
  matches `\mathbb{E}_#1[#2]` and a notation round-trips through Unicode.
- **§9, rewriting.** Where the form supplies the grouping, a binding that is
  itself one group loses its braces first: a hole written `{#1}` or sitting
  after `^` or `_`. `_{x ∼ p}` moved into `\E{#1}{#2}` is `\E{x \sim p}{f}`,
  and writing a form over itself no longer adds a pair per run. A notation
  that is itself a script operand is kept as written and recorded as not
  rewritten. The filled form is converted without inlining macros: a form
  that writes `\R^{#1}` was chosen for its `\R`. A notation kept as written
  starts no other match from its literals, so it is counted once.
- **§11, `--verify`.** Every document is a root, and so is every FILE that
  no document among the FILEs reads, by the project reader: a snippet
  beside a document is verified, where it was silently passed over. Each
  root gets a verdict; a root that cannot be set up or whose original does
  not compile reports `error:` and the run exits 2 after printing the
  others. A document's copy holds the sources under its own directory; a
  changed source it reads from outside is an error, since the copy cannot
  place it, and a source of another document is ignored, so documents in
  sibling directories verify side by side. `--keep DIR` may be relative; a
  build directory carries a marker file, one that has it is cleared before
  use and one that lacks it and is not empty is refused. The directories of
  the definition files are on `TEXINPUTS`. A `\documentclass` in a comment
  makes no document. `--dpi` is held to 50…2400.
- **`declarations`.** The atoms of a notation's literals count: the sources
  are read with no form chosen. (Found by the compile check on the fixture
  for styled literals: the `𝒩` of a rewritten `𝒩(μ, Σ)` went undeclared.)
- **§14.** Thirteen fixtures, one per defect that a fixture can pin, and
  three tests in `tests/verify.rs` for several roots, kept builds and macro
  packages.
- **Ruled on, not changed.** An unterminated region's warning says `left as
  is` while its symbols are still converted, as since version zero; the
  wording is the fault. `--verify` may add fonts to the user's TeX cache
  (bbm is METAFONT only), as any compile does. A `differs` fixture that
  holds several cases can hide one of them changing; the cases are split
  when one does.

### 2026-11-09, a ruling on bold Greek

- **§17, `bf_greek`.** Ruled by the author on the paper's own case:
  `\mathbf{\sigma_\theta}` means all of it bold, the sigma and its
  subscript. The default therefore stands (`bf_greek` is `\boldsymbol`,
  and the style reaches through the script), and where pdflatex drew the
  source plain the conversion's bold is a repair, reported by `--verify` as
  a difference until the source says what it means. Measured: both
  profiles' output sets the pair in CMMIB10 and CMMIB7 where the original
  used CMMI10 and CMMI7, and the paper with its two occurrences written
  `\boldsymbol{\sigma_\theta}` verifies on all fourteen pages under the
  default configuration. `bf_greek = "\\mathbf"` remains for a paper that
  wants pdflatex's reading kept.

### 2026-11-09, the third review

An independent review of project mode and the second review's fixes
(`90834dc..89ed710`) returned twenty-two findings; `docs/REVIEW-2026-11-09.md`
lists them with what was done. The behaviour that changed:

- **§11, `declarations` under unicode-math-input.** Every character is
  declared, not the styled letters alone. Measured: with every symbol of the
  table declared, all 356 render identically to their commands under
  pdflatex, delimiters after `\left`, `\middle` and `\bigl` included; left to
  the package's own table, ten do not compile (`↝`, `⊳`, `Ⓢ`, the black
  triangles) and six render otherwise (`√` as the radical, `∖`, `′`, `‵`,
  `ℏ`, `∅`). The README's "knows every symbol" is withdrawn.
- **§12, `packages.tsv`.** Three columns: the command, what defines it under
  pdflatex, and whether lualatex with unicode-math defines it. The kernel's
  `\not@base` stubs (`\mho`, `\leadsto`, `\sqsubset`, `\sqsupset`) count as
  undefined, so those four are amsfonts'. `\napprox` is defined by no
  pdflatex package: `latex.tsv` keeps it as an input alias and `≉` has no
  command spelling, which the table test now requires of every spelling.
  `\blacktriangleright` and its three kin pointed at the open small
  triangles; they point at the filled ones. `𝕜` is `\Bbbk`, and a lowercase
  blackboard letter is spelled with `bb_lower` (`\mathbbm`), since amsfonts'
  `\mathbb{a}` is another glyph.
- **§9, under unicode-math.** A profile carries its preset. Under
  `unicode-math` a Named atom whose curated command that package lacks
  (`\square`, `\circlearrowleft`, `\blacksquare`, nine in all) is spelled
  by unicode-math's own name (`\mdlgwhtsquare`), which it always defines.
  Under a pdflatex mechanism `declarations` never names `unicode-math` and
  never merges a package into it.
- **§9, engine.** A FILE that is itself a document, named without a root,
  is its own root: its preamble decides the engine and the commands, as
  under `--project`. `--engine` is a selection option, not a `--verify`
  one: it sets the config's `engine` for the run, and the profile's preset,
  the declarations' mechanism and the compile follow one rule (unicode-math
  when the preamble loads it or the engine is not pdflatex).
- **§9, scripts.** A script operator before or after a thing is `^`, `_`,
  `\sp` or `\sb`, with whitespace, newlines and comments between, and a
  prime after a group is `'` or a prime character. The lines before are
  lexed from their start, since only that tells a comment from code.
- **§8, holes.** A hole's text is trimmed: the space `plan` added to keep a
  control word off a letter was the source's seam, not the form's, and made
  a rewrite grow a space per run. The braces of a binding are dropped only
  when it was one group that stood as a script's operand; a group standing
  alone in a hole keeps them (`{-x}`).
- **§7, `\providecommand`.** It defines a name only when the name is not
  yet defined, as in TeX, so a section's fallback does not override the
  package it falls back from.
- **§3, the reader.** Names resolve as TeX's do: `name.tex` before `name`,
  for `\include` too; against the root's directory, or against the import
  path inside a file that `\import` or `\subimport` brought in, where a
  plain `\input` follows the import path. `\endinput` takes effect at the
  end of its line. A byte-order mark is skipped. A `.sty` or `.cls` the
  body inputs is a package. A file nested deeper than 32 or not UTF-8 is
  listed as missing with the reason. A root with no `\documentclass` is a
  snippet: all of it is body and its inputs are files. `is_document` lives
  in the reader, ignores comments and verbatim environments, and is what
  `--verify` uses.
- **§11, `--verify`.** A root is a document or a FILE named for
  verification (every FILE, or a project's root), unless a document among
  the sources reads it: a subfile or section is verified with its document,
  and a project whose root is a snippet is verified. A document's copy
  holds every source it reads, whichever root converted it, so a file two
  roots share and read under different definitions shows the second root
  `differs` where it passed before. The output directory gets a
  subdirectory for every file the document reads, not only the FILEs. The
  declarations go before the `\begin{document}` TeX acts on (`\begin
  {document}` with a space; not in a comment, where `\\%` starts one; not
  in `filecontents` or `verbatim`); a document with none is an error. A
  snippet verified alone under a root is wrapped with the root's local
  packages and preamble inputs, and the project's directory is on
  `TEXINPUTS`.
- **§11, the command line.** Options before a subcommand are refused with
  a message, where clap dropped them.
- **§14.** Fixtures `bb-lowercase`, `bb-lowercase-ascii`, `amsfonts-stubs`,
  `structured-group-before-sp`, `notation-script-operand-newline`,
  `notation-hole-after-control-word`, `notation-standing-group-in-hole`;
  the `paper` project case gains a `\providecommand`; `\mathbb{R^n}` moves
  from `style-distribution` to the repairs case, since `\mathbb{n}` is
  another glyph in amsfonts and the conversion repairs it. Tests for the
  reader's resolution, snippet roots and depth, `is_document`,
  `\providecommand`, script detection, hole spacing and standing groups,
  the engine flag, options before a subcommand, a shared file under two
  roots, and root selection.
- **§12, `∅`.** Found when the paper was checked with every character
  declared: it writes `\varnothing`, and `∅` was declared `\emptyset`, which
  pdflatex draws as a slashed zero where `\varnothing` is the slashed circle
  that the character is. The curated spelling of U+2205 is now
  `\varnothing`, as unicode-math names it; `\emptyset` still reads as the
  same atom and is a fifth collapse, documented in the README.
- **Ruled on, not changed.** The unicode-math preset rewrites `\mathbf`,
  `\mathit`, `\mathsf` and `\mathtt` to `\sym…`, which changes the glyph
  when the text font differs from the math font: this is the documented
  shape collapse under that package, and `--verify` reports it. About five
  thousand nested braces overflow the parser's stack, as since version
  zero. A `\begin{document}` inside `\iffalse…\fi` is still taken as the
  real one. The paper's numbers in the README come from the author's own
  tree, which the reviewer could not read.

### 2026-11-10, the fourth review

An independent review of the third review's fixes (`5270bbd`) returned
seventeen findings, five of them regressions of those fixes;
`docs/REVIEW-2026-11-09.md` lists them with what was done. The behaviour
that changed:

- **§9, what decides the commands.** Whether the document loads
  unicode-math, and nothing else: with a root, its preamble; without one, an
  engine other than pdflatex, since `--verify`'s own snippet preamble for
  lualatex and xelatex loads it. The engine (a fontspec document is lualatex)
  and `unicode_input` no longer do; the amendment of 2026-11-09 tied the
  preset to the mechanism, which gave a fontspec document `\symbf` it could
  not compile. The mechanism follows the same test, and without
  unicode-math it is unicode-math-input, which works under lualatex too
  (measured).
- **§9, the root.** `--root`, else the config's `root`, else, for a FILE
  named on its own, the document itself or the main document a subfile
  names (`\documentclass[../main.tex]{subfiles}`). The amendment of
  2026-11-09 let a file's own `\documentclass` override the config's root.
- **§12, unicode-math's glyphs.** `packages.tsv` gains a fourth column: for
  a command that spells one character, whether lualatex with unicode-math
  typesets the same glyphs for it as for the character, read off the boxes
  with Lua. Four differ (`\backepsilon`, `\blacktriangleleft`,
  `\blacktriangleright`, `\bullet`). Under the unicode-math preset such a
  command is not written for the character (unicode-math's own name is),
  and read back it means what unicode-math draws: its own character where
  its table names one (`\blacktriangleright` is ▶), else no atom, so it is
  left as written. A character with no curated spelling is written by its
  unicode-math name under that preset.
- **§12, the primes.** `′`, `″`, `‴` are spelled `'`, `''`, `'''`, in every
  profile: under unicode-math-input and unicode-math the characters are the
  superscript primes, measured identical to the apostrophes, while
  `\prime` is the unraised glyph. `\prime`, `\dprime`, `\trprime` and
  `\backprime` no longer read as those characters (a fourth column of
  `latex.tsv`, names that are not aliases), and `‵` has no spelling. Under
  the kernel mechanism the primes are undeclarable: a declared prime before
  a script is a double superscript.
- **§12, `defined.tsv`.** Generated with the package table: every
  letters-only name the kernel defines, and every one amsmath, amsfonts,
  amssymb, amsthm, mathtools, bm, bbm, mathrsfs and dsfont add, from
  LuaTeX's hash. A `\providecommand` of a name the kernel or a loaded
  package defines (without a root: the kernel, amsmath, amsfonts, amssymb),
  or that the symbol table or the signatures know, did nothing and is
  dropped.
- **§12, blackboard and the lozenge.** With bbold or mathbbol loaded,
  `bb_digit` and `bb_lower` are the `bb` command and `\Bbbk` is not used. `\blacklozenge`
  is U+29EB, the black lozenge, not U+25C6.
- **§9, scripts.** `\relax` and `\space` before a script are transparent.
  The script test reads the end of a line without lexing it when the line
  holds no `%`, which keeps a long line linear.
- **§8, holes.** A hole's text keeps the space of a trailing control space
  (`b\ `), both when trimmed for the form and when bound as a group's
  contents; without it the rewrite escaped the brace that follows.
- **§3, the reader.** One reading of the live text (comments, verbatim
  environments including one opened and closed on one line, and the
  arguments of `\verb` and `\lstinline` blanked) serves `is_document`, the
  search for `\begin{document}` and the subfile main. Inside an imported
  file a name resolves in the import path, then from the root's directory,
  as TeX does. A name ending in `.tex` is not extended again.
- **§11, `--verify`.** Every file the root reads is copied, as converted
  where mathfmt converted it, under the files' common ancestor: a name
  through `../` finds its file, a snippet root is compiled with its inputs,
  and a subfile compiled alone has its main document. The snippet wrapper
  is written as `mathfmt-snippet.tex`, so it never shadows a source. A
  local class goes in place of `article` in the wrapper. A document that
  reads a changed file outside its directory is no longer an error.
- **Lint.** When a root is known, a conversion that writes a command the
  preamble does not provide (a package it does not load, or under
  unicode-math a name that package lacks) is a `package-not-loaded`
  warning. A package mathfmt does not know the contents of may provide it.
- **§14.** Fixtures `prime-is-apostrophe`, `structured-group-before-relax`,
  `notation-hole-control-space`; the project case `bbold`; the
  `unicode-math` case gains the shapes. Tests for the import fallback, live
  text, the preamble deciding the commands, a root in force beating a
  file's class, the lint, and a root compiled with everything it reads.
  The flake's `verify` check gains bbold.
- **Measured.** The table sweep, every symbol on its own page through
  `--project --verify --declare`: 357 of 357 identical under
  unicode-math-input in both profiles, under the kernel in the unicode
  profile, and under unicode-math in the ascii profile; under the kernel in
  the ascii profile 354 of 354 once the three primes are left out, and under
  unicode-math in the unicode profile 348 of 348 once the nine amssymb names
  unicode-math lacks are left out (an original spelled that way does not
  compile there).
- **Ruled on, not changed.** `\InputIfFileExists{x}{a}{b}` lists both
  branches. `e^\alpha x` matched by `e^#1 x` writes `e^{{α}} x`, stable and
  rendering the same. Five thousand nested braces still overflow the stack.

### 2026-11-10, the Python distribution

- **§16, distribution.** A Python package, `mathfmt-tex`, whose wheels hold
  the compiled binary and nothing else (`pyproject.toml`, maturin with
  `bindings = "bin"`, the version Cargo's): `pip`, `pipx` or `uv tool`
  install a `mathfmt` command with no Rust toolchain. `mathfmt` on PyPI is
  an unrelated project (Word equations), hence the name; the command keeps
  its name, and a machine with both would have two `mathfmt` commands.
  This replaces `cargo-dist` in the plan: the wheel is the binary, and pip
  already knows how to pick one per platform.
- **Wheels.** `.github/workflows/wheels.yml` runs on the GitHub mirror only
  (every job checks `github.server_url`, so a Forgejo that reads
  `.github/workflows` skips it): manylinux and musllinux for x86-64 and
  ARM, macOS Intel and Apple, Windows x64, and an sdist; each native wheel
  is installed and run. A `v*` tag publishes to PyPI by trusted publishing,
  which needs the PyPI project to name the repository, the workflow and the
  `pypi` environment first. Its first run on GitHub, on `f7d3508`, built
  and ran every wheel but Apple silicon, whose runner label `macos-14` is
  retired; that job runs on `macos-15` now.
- **pre-commit without Rust.** A `repo: local` hook with
  `language: python` and `additional_dependencies: ["mathfmt-tex==…"]`:
  pre-commit installs the wheel into its own environment. A hook repo with
  `language: python` would not do, since pre-commit installs the hook repo
  itself from source, which with maturin means cargo. The `language: rust`
  hooks stay for those who have it.
- **§14.** A flake check `wheel` builds the wheel offline from the vendored
  crates (nixpkgs' `maturinBuildHook`), installs it and runs its `mathfmt`
  on a line of math. `tests/pre-commit-e2e.sh`, run by hand since it needs
  the network for pre-commit, builds the wheel and runs the local hook in a
  scratch repository: the first run rewrites the file and fails, the second
  passes.
