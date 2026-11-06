//! mathfmt-core: rewrite math notation in LaTeX source.
//!
//! Pipeline, per file: [`parse::scan`] finds math regions and definitions,
//! [`resolve::resolve`] turns them into atoms, [`print::plan`] decides the
//! edits a profile wants, and [`print::apply`] writes them into the original
//! bytes. [`Engine`] holds the tables and the config and runs all four.
//!
//! See `docs/superpowers/specs/2026-11-05-mathfmt-design.md`.

pub mod atom;
pub mod config;
pub mod diag;
pub mod lex;
pub mod macros;
pub mod parse;
pub mod print;
pub mod resolve;
pub mod signatures;
pub mod table;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

pub use atom::{Atom, Style};
pub use config::{Config, ConfigError, Form, Profile};
pub use diag::{Diagnostic, Level, LineIndex};
pub use lex::Span;
pub use macros::{Macro, MacroTable};
pub use parse::{scan, Document, MathRegion, Node, RegionKind, ScanOptions};
pub use print::{Edit, RenameMap};
pub use resolve::{OccKind, Occurrence};
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
    pub document: Document,
    pub diagnostics: Vec<Diagnostic>,
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
    /// Definitions from the config's and the command line's macro files.
    pub macros: MacroTable,
}

impl Engine {
    pub fn new(config: Config) -> Result<Engine, Error> {
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
            config,
        };
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
        let doc = scan(
            text,
            &self.signatures,
            &self.scan_options,
            self.macros.clone(),
        );
        self.macros = doc.macros;
    }

    /// The profile to use: `name`, else the config's default.
    pub fn profile(&self, name: Option<&str>) -> Result<Profile, Error> {
        Ok(self
            .config
            .profile(name.unwrap_or(self.config.default_profile_name()))?)
    }

    /// Convert one file's text.
    pub fn convert(&self, src: &str, profile: &Profile, rename: &RenameMap) -> Outcome {
        let document = scan(
            src,
            &self.signatures,
            &self.scan_options,
            self.macros.clone(),
        );
        let mut diagnostics = document.diagnostics.clone();
        let (occurrences, more) =
            resolve::resolve(&document, src, &self.table, &self.signatures, profile);
        diagnostics.extend(more);
        let (edits, more) = print::plan(src, &occurrences, profile, &self.table, rename);
        diagnostics.extend(more);
        let output = print::apply(src, &edits);
        diagnostics.sort_by_key(|d| d.offset);
        Outcome {
            changed: output != src,
            output,
            edits,
            occurrences,
            document,
            diagnostics,
        }
    }

    /// The notation inventory of one file: each atom, how often, spelled how.
    pub fn inventory(&self, src: &str, profile: &Profile) -> Vec<SymbolCount> {
        let document = scan(
            src,
            &self.signatures,
            &self.scan_options,
            self.macros.clone(),
        );
        let (occurrences, _) =
            resolve::resolve(&document, src, &self.table, &self.signatures, profile);
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
        let document = scan(
            &src,
            &self.signatures,
            &self.scan_options,
            self.macros.clone(),
        );
        let (occurrences, _) =
            resolve::resolve(&document, &src, &self.table, &self.signatures, profile);
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
