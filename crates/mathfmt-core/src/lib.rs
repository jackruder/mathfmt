//! mathfmt-core: rewrite math notation in LaTeX source.
//!
//! Pipeline, per file: [`parse::scan`] finds math regions and definitions,
//! [`resolve::resolve`] turns them into atoms, [`print::plan`] decides the
//! edits a profile wants, and [`print::apply`] writes them into the original
//! bytes. [`Engine`] holds the tables and the config and runs all four.
//! With a root document ([`project::Project`]) it also knows the files, the
//! definitions and the packages of the whole project, and
//! [`Engine::declarations`] says what the preamble must hold.
//!
//! See `docs/superpowers/specs/2026-11-05-mathfmt-design.md`.

pub mod atom;
pub mod config;
pub mod declare;
pub mod diag;
pub mod lex;
pub mod macros;
pub mod notation;
pub mod parse;
pub mod print;
pub mod project;
pub mod resolve;
pub mod signatures;
pub mod table;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

pub use atom::{Atom, Style};
pub use config::{Config, ConfigError, Form, Profile, StylePreset};
pub use declare::{CharDecl, Declarations, Mechanism};
pub use diag::{Diagnostic, Level, LineIndex};
pub use lex::Span;
pub use macros::{Macro, MacroTable};
pub use notation::NotationOccurrence;
pub use parse::{scan, Document, MathRegion, Node, RegionKind, ScanOptions};
pub use print::{Edit, RenameMap};
pub use project::Project;
pub use resolve::{OccKind, Occurrence, Part};
pub use signatures::Signatures;
pub use table::Table;

#[derive(Debug)]
pub enum Error {
    Config(ConfigError),
    Io(PathBuf, std::io::Error),
    /// A spelling given on the command line is not exactly one atom.
    NotOneAtom(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Config(e) => write!(f, "config: {e}"),
            Error::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Error::NotOneAtom(s) => write!(f, "`{s}` does not spell exactly one symbol"),
        }
    }
}

impl std::error::Error for Error {}

impl From<ConfigError> for Error {
    fn from(e: ConfigError) -> Self {
        Error::Config(e)
    }
}

/// What converting one file produced.
#[derive(Debug)]
pub struct Outcome {
    pub output: String,
    pub changed: bool,
    pub edits: Vec<Edit>,
    pub occurrences: Vec<Occurrence>,
    /// The notations recognized, rewritten or not.
    pub notations: Vec<NotationOccurrence>,
    pub document: Document,
    pub diagnostics: Vec<Diagnostic>,
}

/// One notation in a file, with how often and in which forms: the inventory.
#[derive(Debug, Clone)]
pub struct NotationCount {
    pub name: String,
    pub count: usize,
    pub forms: BTreeMap<String, usize>,
}

/// One distinct atom in a file, with how it was spelled: the inventory.
#[derive(Debug, Clone)]
pub struct SymbolCount {
    pub atom: Atom,
    pub unicode: Option<char>,
    pub name: Option<String>,
    pub count: usize,
    pub spellings: BTreeMap<String, usize>,
}

/// The tables, the config and the definitions in force. Build one per run and
/// share it across files; it is read-only after construction.
#[derive(Debug)]
pub struct Engine {
    pub table: Table,
    pub signatures: Signatures,
    pub config: Config,
    pub scan_options: ScanOptions,
    /// Definitions from the project's preamble and from the config's and
    /// the command line's macro files.
    pub macros: MacroTable,
    /// The root document and what it pulls in, when one is known.
    pub project: Option<Project>,
}

impl Engine {
    /// An engine for this config; its `root`, when it names one, is read.
    pub fn new(config: Config) -> Result<Engine, Error> {
        Engine::with_root(config, None)
    }

    /// An engine whose project is `root` (as the caller names it), or the
    /// config's `root` when `None`. The definitions of the root's preamble
    /// and of the local packages it loads come first, so the config's macro
    /// files, then the command line's, then each file's own, override them.
    pub fn with_root(config: Config, root: Option<&Path>) -> Result<Engine, Error> {
        let table = Table::builtin();
        let mut signatures = Signatures::builtin();
        for c in &config.text_commands {
            signatures.add_text_command(c);
        }
        let mut scan_options = ScanOptions::default();
        scan_options
            .math_environments
            .extend(config.math_environments.iter().cloned());
        scan_options
            .verbatim_environments
            .extend(config.verbatim_environments.iter().cloned());
        let mut engine = Engine {
            table,
            signatures,
            scan_options,
            macros: MacroTable::new(),
            project: None,
            config,
        };
        if let Some(root) = root
            .map(Path::to_path_buf)
            .or_else(|| engine.config.root_path())
        {
            let project = Project::read(&root).map_err(|e| Error::Io(root.clone(), e))?;
            for f in &project.definitions {
                engine.add_macro_file(f)?;
            }
            engine.add_macro_file(&root)?;
            engine.project = Some(project);
        }
        let files: Vec<PathBuf> = engine
            .config
            .macros
            .iter()
            .map(|m| match &engine.config.dir {
                Some(d) => d.join(m),
                None => PathBuf::from(m),
            })
            .collect();
        for f in files {
            engine.add_macro_file(&f)?;
        }
        Ok(engine)
    }

    /// Read a file for its definitions only.
    pub fn add_macro_file(&mut self, path: &Path) -> Result<(), Error> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::Io(path.to_path_buf(), e))?;
        self.add_macro_source(&text);
        Ok(())
    }

    pub fn add_macro_source(&mut self, text: &str) {
        let doc = scan(text, &self.signatures, &self.scan_options, &self.macros);
        self.macros = doc.macros;
    }

    /// The profile to use: `name`, else the config's default. A profile
    /// that does not choose a `style_preset` gets unicode-math's commands
    /// when the document reads Unicode through unicode-math (the preamble
    /// loads it, or the engine is not pdflatex): the same rule the
    /// declarations follow, so the two never disagree.
    pub fn profile(&self, name: Option<&str>) -> Result<Profile, Error> {
        let preset = match self.mechanism() {
            Mechanism::UnicodeMath => StylePreset::UnicodeMath,
            _ => StylePreset::Latex,
        };
        Ok(self
            .config
            .profile_with(name.unwrap_or(self.config.default_profile_name()), preset)?)
    }

    /// The program that compiles the document: the config's `engine`, else
    /// what the project's root says, else `pdflatex`.
    pub fn program(&self) -> String {
        match (&self.config.engine, &self.project) {
            (Some(e), _) => e.clone(),
            (None, Some(p)) => p.engine().to_string(),
            (None, None) => "pdflatex".to_string(),
        }
    }

    /// How the document reads Unicode math: the config's `unicode_input`,
    /// else `unicode-math` when the preamble loads it or the engine is not
    /// pdflatex, else `unicode-math-input`.
    pub fn mechanism(&self) -> Mechanism {
        if let Some(m) = self.config.unicode_input {
            return m;
        }
        let unicode_math = self
            .project
            .as_ref()
            .is_some_and(|p| p.loads("unicode-math"));
        if unicode_math || !matches!(self.program().as_str(), "pdflatex" | "latex") {
            Mechanism::UnicodeMath
        } else {
            Mechanism::UnicodeMathInput
        }
    }

    /// Every atom these sources use, before or after conversion. The
    /// literals of a notation count: they are read with no form chosen, so
    /// a match is walked like anything else instead of being replaced.
    fn atoms_used(&self, sources: &[&str], profile: &Profile) -> BTreeSet<Atom> {
        let reading = Profile {
            notations: Default::default(),
            ..profile.clone()
        };
        let none = RenameMap::new();
        let mut atoms = BTreeSet::new();
        for src in sources {
            let converted = self.convert(src, profile, &none).output;
            for text in [*src, converted.as_str()] {
                for occ in &self.convert(text, &reading, &none).occurrences {
                    atoms.extend(occ.atoms.iter().copied());
                }
            }
        }
        atoms
    }

    /// What the preamble must hold for these sources to compile in either
    /// form: the packages the profile's commands come from, and a
    /// declaration for each Unicode character the notation has, meaning
    /// what the profile spells it as with commands. `mechanism` is how the
    /// document reads Unicode ([`Engine::mechanism`] when `None`).
    pub fn declarations(
        &self,
        sources: &[&str],
        profile: &Profile,
        mechanism: Option<Mechanism>,
    ) -> Declarations {
        let mechanism = mechanism.unwrap_or_else(|| self.mechanism());
        // The spellings the characters are declared to mean come from the
        // profile with every form forced to commands; under unicode-math
        // they are that package's names where it lacks amssymb's.
        let mut reference = profile.latex_forms();
        if mechanism == Mechanism::UnicodeMath {
            reference.preset = StylePreset::UnicodeMath;
        }
        let pdflatex = mechanism != Mechanism::UnicodeMath;
        let atoms = self.atoms_used(sources, profile);
        let mut packages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut chars: BTreeSet<char> = BTreeSet::new();
        for atom in &atoms {
            chars.extend(self.table.char_of(atom));
            if !pdflatex {
                continue;
            }
            let Some(spelling) = print::spelling(atom, &reference, &self.table) else {
                continue;
            };
            for word in declare::control_words(&spelling) {
                match self.table.package_of(&word) {
                    // A command no pdflatex package defines is reported
                    // with its character below, not as a package.
                    Some("-") => {}
                    Some(package) => {
                        packages
                            .entry(package.to_string())
                            .or_default()
                            .insert(word);
                    }
                    None => {}
                }
            }
        }
        let mut decls = Vec::new();
        let mut undeclared = Vec::new();
        for c in chars {
            if !pdflatex {
                break;
            }
            // The character as the resolver reads it, which is what a
            // conversion would write for it.
            let Ok(atom) = self.parse_atom(&c.to_string(), profile) else {
                continue;
            };
            match print::spelling(&atom, &reference, &self.table) {
                Some(s) if s.is_ascii() => {
                    let unavailable: Vec<String> = declare::control_words(&s)
                        .into_iter()
                        .filter(|w| self.table.defined_by(w) == Some("-"))
                        .collect();
                    if !unavailable.is_empty() {
                        undeclared.push((
                            c,
                            format!(
                                "its spelling `{s}` uses \\{}, which no pdflatex package defines",
                                unavailable.join(", \\")
                            ),
                        ));
                    } else {
                        // unicode-math-input's own table lacks some symbols
                        // (↝) and reads others differently (√ as a radical),
                        // so every character is declared under it too.
                        decls.push(CharDecl {
                            ch: c,
                            atom,
                            spelling: s,
                        });
                    }
                }
                Some(_) => undeclared.push((
                    c,
                    "the profile spells it with a character that is not ASCII".to_string(),
                )),
                None => undeclared.push((
                    c,
                    "the tables have no LaTeX spelling for it; declare it by hand".to_string(),
                )),
            }
        }
        match mechanism {
            Mechanism::UnicodeMathInput if !atoms.is_empty() => {
                packages
                    .entry("unicode-math-input".to_string())
                    .or_default();
            }
            Mechanism::UnicodeMath if !atoms.is_empty() => {
                packages.entry("unicode-math".to_string()).or_default();
            }
            _ => {}
        }
        if let Some(project) = &self.project {
            packages.retain(|package, _| !project.loads(package));
        }
        // A package another needed one loads is named by that one's line.
        let names: Vec<String> = packages.keys().cloned().collect();
        for name in &names {
            for inner in project::implied(name) {
                if let Some(commands) = packages.remove(*inner) {
                    packages.entry(name.clone()).or_default().extend(commands);
                }
            }
        }
        Declarations {
            mechanism,
            preamble_known: self.project.is_some(),
            packages,
            chars: decls,
            undeclared,
        }
    }

    /// Convert one file's text.
    pub fn convert(&self, src: &str, profile: &Profile, rename: &RenameMap) -> Outcome {
        let document = scan(src, &self.signatures, &self.scan_options, &self.macros);
        let mut diagnostics = document.diagnostics.clone();
        let decls = self.config.notation_decls();
        let notations = notation::parse_notations(&decls, &self.signatures, &document.macros);
        let (occurrences, matches, more) = resolve::resolve(
            &document,
            src,
            &self.table,
            &self.signatures,
            profile,
            &notations,
        );
        diagnostics.extend(more);
        let (edits, more) = print::plan(src, &occurrences, profile, &self.table, rename);
        diagnostics.extend(more);
        let edits = if matches.is_empty() {
            edits
        } else {
            let convert =
                |fragment: &str| self.convert_fragment(fragment, &document.macros, profile, rename);
            notation::rewrite(src, edits, &matches, &notations, profile, &convert)
        };
        let records: Vec<NotationOccurrence> = matches
            .iter()
            .map(|m| {
                let n = &notations[m.notation];
                NotationOccurrence {
                    name: n.name.clone(),
                    form: n.forms[m.form].name.clone(),
                    span: m.span,
                    holes: m.holes.iter().map(|b| b.span).collect(),
                    rewritten: m.rewritten,
                }
            })
            .collect();
        drop(matches);
        let output = print::apply(src, &edits);
        diagnostics.sort_by_key(|d| d.offset);
        Outcome {
            changed: output != src,
            output,
            edits,
            occurrences,
            notations: records,
            document,
            diagnostics,
        }
    }

    /// Convert a rendered notation form (math only, no notations) so its
    /// literals come out in the profile's spellings. Macros are not inlined
    /// here: a form that writes `\R` was chosen for it, and the holes were
    /// already converted under the profile's own setting.
    fn convert_fragment(
        &self,
        text: &str,
        macros: &MacroTable,
        profile: &Profile,
        rename: &RenameMap,
    ) -> String {
        let (nodes, _) = parse::parse_math_str(text, &self.signatures, macros);
        let doc = Document {
            regions: vec![MathRegion {
                kind: RegionKind::Inline,
                open: Span::new(0, 0),
                close: None,
                nodes,
            }],
            macros: macros.clone(),
            diagnostics: Vec::new(),
        };
        let literal = Profile {
            inline_macros: false,
            ..profile.clone()
        };
        let (occurrences, _, _) =
            resolve::resolve(&doc, text, &self.table, &self.signatures, &literal, &[]);
        let (edits, _) = print::plan(text, &occurrences, &literal, &self.table, rename);
        print::apply(text, &edits)
    }

    /// The notation inventory of one file: each notation, how often, in
    /// which forms.
    pub fn notation_inventory(&self, src: &str, profile: &Profile) -> Vec<NotationCount> {
        let outcome = self.convert(src, profile, &RenameMap::new());
        let mut map: BTreeMap<String, NotationCount> = BTreeMap::new();
        for n in &outcome.notations {
            let entry = map.entry(n.name.clone()).or_insert_with(|| NotationCount {
                name: n.name.clone(),
                count: 0,
                forms: BTreeMap::new(),
            });
            entry.count += 1;
            *entry.forms.entry(n.form.clone()).or_insert(0) += 1;
        }
        map.into_values().collect()
    }

    /// The notation inventory of one file: each atom, how often, spelled how.
    pub fn inventory(&self, src: &str, profile: &Profile) -> Vec<SymbolCount> {
        let document = scan(src, &self.signatures, &self.scan_options, &self.macros);
        let (occurrences, _, _) =
            resolve::resolve(&document, src, &self.table, &self.signatures, profile, &[]);
        let mut map: BTreeMap<Atom, SymbolCount> = BTreeMap::new();
        for occ in &occurrences {
            let spelling = occ.span.text(src).to_string();
            for atom in &occ.atoms {
                let entry = map.entry(*atom).or_insert_with(|| SymbolCount {
                    atom: *atom,
                    unicode: self.table.char_of(atom),
                    name: self
                        .table
                        .char_of(atom)
                        .and_then(|c| self.table.info(c))
                        .and_then(|i| i.name.clone()),
                    count: 0,
                    spellings: BTreeMap::new(),
                });
                entry.count += 1;
                *entry.spellings.entry(spelling.clone()).or_insert(0) += 1;
            }
        }
        map.into_values().collect()
    }

    /// Resolve a spelling from the command line (`ℱ`, `\mathfrak{F}`, `\R`)
    /// to exactly one atom.
    pub fn parse_atom(&self, spelling: &str, profile: &Profile) -> Result<Atom, Error> {
        let src = format!("${spelling}$");
        let document = scan(&src, &self.signatures, &self.scan_options, &self.macros);
        let (occurrences, _, _) =
            resolve::resolve(&document, &src, &self.table, &self.signatures, profile, &[]);
        let atoms: Vec<Atom> = occurrences.into_iter().flat_map(|o| o.atoms).collect();
        match atoms.as_slice() {
            [a] => Ok(*a),
            _ => Err(Error::NotOneAtom(spelling.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_round_trip_with_macros_from_config() {
        let mut engine = Engine::new(Config::default()).unwrap();
        engine.add_macro_source(r"\newcommand{\R}{\mathbb{R}} \newcommand{\E}{\mathbb{E}}");
        let unicode = engine.profile(Some("unicode")).unwrap();
        let ascii = engine.profile(Some("ascii")).unwrap();
        let src = r"Let $x \in \R$ with $\E[x] = \mu$.";
        let u = engine.convert(src, &unicode, &RenameMap::new());
        assert_eq!(u.output, "Let $x ∈ ℝ$ with $𝔼[x] = μ$.");
        let a = engine.convert(&u.output, &ascii, &RenameMap::new());
        assert_eq!(
            a.output,
            r"Let $x \in \mathbb{R}$ with $\mathbb{E}[x] = \mu$."
        );
        let back = engine.convert(&a.output, &unicode, &RenameMap::new());
        assert_eq!(back.output, u.output);
    }

    #[test]
    fn notations_rewrite_both_ways_and_nest() {
        let config = Config::parse(
            r#"
            [notations.KL]
            macro = "\\KL{#1}{#2}"
            expanded = "D_{\\mathrm{KL}}(#1 \\| #2)"
            [notations.N]
            macro = "\\N(#1, #2)"
            expanded = "\\mathcal{N}(#1, #2)"
            [profiles.ascii.notations]
            KL = "macro"
            N = "macro"
            [profiles.unicode.notations]
            KL = "expanded"
            N = "expanded"
            "#,
            None,
        )
        .unwrap();
        let engine = Engine::new(config).unwrap();
        let ascii = engine.profile(Some("ascii")).unwrap();
        let unicode = engine.profile(Some("unicode")).unwrap();
        let defs = "\\newcommand{\\KL}[2]{D_{\\mathrm{KL}}(#1 \\| #2)}\\newcommand{\\N}[2]{\\mathcal{N}(#1, #2)}\n";
        let src = format!(
            "{defs}$D_{{\\mathrm{{KL}}}}(\\mathcal{{N}}(\\mu, \\Sigma) ‖ q_\\phi) \\le \\alpha\\KL{{p}}{{q}}$"
        );
        let a = engine.convert(&src, &ascii, &RenameMap::new());
        assert_eq!(
            a.output,
            format!("{defs}$\\KL{{\\N(\\mu, \\Sigma)}}{{q_\\phi}} \\leq \\alpha\\KL{{p}}{{q}}$"),
            "to the macro forms, nested, with the inner hole converted"
        );
        assert_eq!(a.notations.len(), 3);
        assert!(a.notations.iter().all(|n| n.rewritten));
        let u = engine.convert(&src, &unicode, &RenameMap::new());
        assert_eq!(
            u.output,
            format!("{defs}$D_{{\\mathrm{{KL}}}}(𝒩(μ, Σ) ‖ q_{{ϕ}}) ≤ α D_{{\\mathrm{{KL}}}}(p ‖ q)$"),
            "to the expanded forms, the form's own \\| in the profile's spelling, a space after \\alpha"
        );
        for (out, p) in [(&a.output, &ascii), (&u.output, &unicode)] {
            let again = engine.convert(out, p, &RenameMap::new());
            assert_eq!(&again.output, out, "idempotent under {}", p.name);
        }
        let braced = engine.convert(
            &format!("{defs}$\\underbrace{{\\KL{{p}}{{q}}}}_{{x}} + {{\\KL{{p}}{{q}}}}$"),
            &unicode,
            &RenameMap::new(),
        );
        assert_eq!(
            braced.output,
            format!("{defs}$\\underbrace{{D_{{\\mathrm{{KL}}}}(p ‖ q)}}_{{x}} + {{D_{{\\mathrm{{KL}}}}(p ‖ q)}}$"),
            "a notation inside braces keeps them"
        );
        // A profile with no choice leaves the form alone but still sees it.
        let plain = Engine::new(Config::parse(
            "[notations.KL]\nmacro = \"\\\\KL{#1}{#2}\"\nexpanded = \"D_{\\\\mathrm{KL}}(#1 \\\\| #2)\"",
            None,
        )
        .unwrap())
        .unwrap();
        let p = plain.profile(Some("unicode")).unwrap();
        let o = plain.convert(r"$D_{\mathrm{KL}}(p \| q)$", &p, &RenameMap::new());
        assert_eq!(o.output, "$D_{\\mathrm{KL}}(p ‖ q)$");
        assert_eq!((o.notations.len(), o.notations[0].rewritten), (1, false));
        let inv = plain.notation_inventory(r"$D_{\mathrm{KL}}(p \| q) + \KL{a}{b}$", &p);
        assert_eq!((inv[0].count, inv[0].forms.len()), (2, 2));
    }

    /// Every math-alphabet command the signatures read has a row in the
    /// generated `packages.tsv`, so `declarations` can name its package.
    #[test]
    fn every_style_command_has_a_package_row() {
        let engine = Engine::new(Config::default()).unwrap();
        let mut names: Vec<&str> = engine.signatures.style_commands().collect();
        names.sort_unstable();
        assert!(names.len() > 30, "{} style commands", names.len());
        for name in names {
            let known = engine.table.defined_by(name).is_some_and(|s| s != "-")
                || (engine.table.defined_by(name).is_some() && engine.table.in_unicode_math(name));
            assert!(
                known,
                "\\{name} has no row in packages.tsv, or neither a pdflatex package nor unicode-math defines it; run tools/gen-packages.py"
            );
        }
    }

    #[test]
    fn declarations_follow_the_mechanism_and_the_profile() {
        let engine = Engine::new(Config::default()).unwrap();
        let unicode = engine.profile(None).unwrap();
        let src = r"$\mathbf{\sigma} \in \mathbb{R}$ and $\mathscr{F} \neq ∆$";
        let umi = engine.declarations(&[src], &unicode, None);
        assert_eq!(umi.mechanism, Mechanism::UnicodeMathInput);
        assert!(!umi.preamble_known);
        let lines = |d: &Declarations| -> Vec<String> {
            d.chars
                .iter()
                .map(|c| format!("{}={}", c.ch, c.spelling))
                .collect()
        };
        // Every character, the script letter reading back as cal.
        assert_eq!(
            lines(&umi),
            [
                r"ℝ=\mathbb{R}",
                r"ℱ=\mathcal{F}",
                r"∈=\in",
                r"≠=\neq",
                r"𝛔=\boldsymbol{\sigma}"
            ]
        );
        assert_eq!(
            umi.packages.keys().collect::<Vec<_>>(),
            ["amsfonts", "amsmath", "mathrsfs", "unicode-math-input"]
        );
        assert_eq!(umi.undeclared.len(), 1, "∆ has no LaTeX spelling");
        let kernel = engine.declarations(&[src], &unicode, Some(Mechanism::Kernel));
        assert_eq!(
            lines(&kernel),
            [
                r"ℝ=\mathbb{R}",
                r"ℱ=\mathcal{F}",
                r"∈=\in",
                r"≠=\neq",
                r"𝛔=\boldsymbol{\sigma}"
            ]
        );
        assert!(!kernel.packages.contains_key("unicode-math-input"));
        // unicode-math reads the characters itself.
        let native = engine.declarations(&[src], &unicode, Some(Mechanism::UnicodeMath));
        assert!(native.chars.is_empty());
        assert_eq!(native.packages.keys().collect::<Vec<_>>(), ["unicode-math"]);
        assert!(engine.declarations(&["no math"], &unicode, None).is_empty());
    }

    fn engine_with(config: &str) -> Engine {
        Engine::new(Config::parse(config, None).unwrap()).unwrap()
    }

    #[test]
    fn a_notation_round_trips_through_styled_literals_and_scripts() {
        let engine = engine_with(
            r#"
            [notations.E]
            macro = "\\E{#1}{#2}"
            sub = "\\mathbb{E}_#1[#2]"
            [notations.N]
            macro = "\\N(#1, #2)"
            expanded = "\\mathcal{N}(#1, #2)"
            [profiles.ascii.notations]
            E = "macro"
            N = "macro"
            [profiles.unicode.notations]
            E = "sub"
            N = "expanded"
            "#,
        );
        let ascii = engine.profile(Some("ascii")).unwrap();
        let unicode = engine.profile(Some("unicode")).unwrap();
        let none = RenameMap::new();
        let u = "$𝔼_{x ∼ p}[f] + 𝒩(μ, Σ)$";
        let a = r"$\E{x \sim p}{f} + \N(\mu, \Sigma)$";
        assert_eq!(
            engine.convert(u, &ascii, &none).output,
            a,
            "the characters are the form's styled literals, and the script's braces are not kept as a second pair"
        );
        assert_eq!(
            engine
                .convert(
                    r"$\mathbb{E}_{x \sim p}[f] + \mathcal{N}(\mu, \Sigma)$",
                    &ascii,
                    &none
                )
                .output,
            a
        );
        assert_eq!(engine.convert(a, &unicode, &none).output, u);
        for (text, profile) in [(u, &unicode), (a, &ascii)] {
            assert_eq!(engine.convert(text, profile, &none).output, text, "stable");
        }
    }

    #[test]
    fn a_notation_is_kept_where_a_rewrite_could_not_be_trusted() {
        let config = r#"
            [notations.KL]
            macro = "\\KL{#1}{#2}"
            expanded = "D_{\\mathrm{KL}}(#1 \\| #2)"
            [notations.Rn]
            short = "\\R^{#1}"
            long = "\\mathbb{R}^{#1}"
            [profiles.unicode.notations]
            KL = "expanded"
            Rn = "short"
            "#;
        let engine = engine_with(config);
        let unicode = engine.profile(Some("unicode")).unwrap();
        let none = RenameMap::new();
        let out = |src: &str| engine.convert(src, &unicode, &none);
        // An unclosed group: the match would run to the end of the file.
        assert_eq!(out("$\\KL{p}{q$ and text").output, "$\\KL{p}{q$ and text");
        // A comment: the rewrite would swallow what follows it.
        let commented = "$\\KL{p % first\n}{q}$";
        assert_eq!(out(commented).output, commented);
        assert_eq!(
            out("$D_{\\mathrm{KL}}(p % c\n\\| q)$").output,
            "$D_{\\mathrm{KL}}(p % c\n‖ q)$",
            "left as written, its symbols still converted"
        );
        // A script operand: what the script takes is the macro's business.
        let script = out(r"$e^\KL{p}{q} + \KL{p}{q}$");
        assert_eq!(script.output, r"$e^\KL{p}{q} + D_{\mathrm{KL}}(p ‖ q)$");
        assert_eq!(
            script
                .notations
                .iter()
                .map(|n| n.rewritten)
                .collect::<Vec<_>>(),
            [false, true]
        );
        // The form a profile picks is written as it is: its \R is not
        // inlined, though a lone \R still is.
        let defs = "\\newcommand{\\R}{\\mathbb{R}}\n";
        assert_eq!(
            out(&format!("{defs}$\\mathbb{{R}}^{{n}} + \\R^{{d}} + \\R$")).output,
            format!("{defs}$\\R^{{n}} + \\R^{{d}} + ℝ$")
        );
        // A form of holes alone, or with a hole at its edge, is refused.
        for bad in [
            "[notations.w]\nbare = \"#1\"",
            "[notations.ip]\ndot = \"#1 \\\\cdot #2\"",
            "[notations.w]\nopen = \"\\\\foo{#1\"",
        ] {
            assert!(Config::parse(bad, None).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_hole_keeps_no_context_spacing_and_a_standing_group_keeps_its_braces() {
        let engine = engine_with(
            r#"
            [notations.ip]
            angle = "\\langle #1, #2 \\rangle"
            bk = "\\bk{#1}{#2}"
            [notations.E]
            macro = "\\E{#1}{#2}"
            sub = "\\mathbb{E}_#1[#2]"
            [profiles.unicode.notations]
            ip = "angle"
            [profiles.ascii.notations]
            ip = "bk"
            E = "macro"
            "#,
        );
        let unicode = engine.profile(Some("unicode")).unwrap();
        let ascii = engine.profile(Some("ascii")).unwrap();
        let none = RenameMap::new();
        // The space `plan` put after `\langle` to keep it off `\alpha` is
        // the source's business, not the hole's.
        let once = engine
            .convert(r"$\langle\alpha, b\rangle$", &unicode, &none)
            .output;
        assert_eq!(once, "$⟨ α, b ⟩$");
        assert_eq!(
            engine.convert(&once, &unicode, &none).output,
            once,
            "stable"
        );
        // A group standing in a hole stays a group; one that was a script's
        // operand sheds the script's braces.
        assert_eq!(
            engine
                .convert(
                    r"$\langle a, {-x} \rangle + \mathbb{E}_{x \sim p}[f]$",
                    &ascii,
                    &none
                )
                .output,
            r"$\bk{a}{{-x}} + \E{x \sim p}{f}$"
        );
    }

    #[test]
    fn a_notation_kept_as_written_is_counted_once() {
        let engine = engine_with("[notations.abs]\nbars = \"|#1|\"\nmacro = \"\\\\abs{#1}\"");
        let p = engine.profile(None).unwrap();
        let inv = engine.notation_inventory("$|a|b|$ and $|(|x|)|$", &p);
        assert_eq!(
            (inv[0].count, inv[0].forms["bars"]),
            (3, 3),
            "|a| once, not |b| again from its closing bar; the nested |x| inside the hole of |(|x|)| is its own"
        );
    }

    #[test]
    fn rename_and_inventory() {
        let engine = Engine::new(Config::default()).unwrap();
        let p = engine.profile(None).unwrap();
        let from = engine.parse_atom(r"\mathfrak{F}", &p).unwrap();
        let to = engine.parse_atom("ℱ", &p).unwrap();
        let mut rename = RenameMap::new();
        rename.insert(from, to);
        let out = engine.convert(r"$\mathfrak{F} \ni 𝔉$", &p, &rename);
        assert_eq!(out.output, "$ℱ ∋ ℱ$");
        let inv = engine.inventory(r"$\mathbb{R} \R ℝ$", &p);
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].count, 2, "\\R is undefined here and so not an atom");
        assert!(engine.parse_atom("xy", &p).is_err());
    }
}
