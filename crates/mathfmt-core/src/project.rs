//! Project mode: what a root document pulls in.
//!
//! A LaTeX project is a root file (the one holding `\documentclass`), the
//! files it reads with `\input` and `\include`, and a preamble that loads
//! packages, some of them local files beside the root. Reading that much,
//! without running TeX, tells mathfmt which files to convert, which
//! definitions are in force in all of them, and which engine and packages
//! the document is written for.
//!
//! The reader is a token walk, not an interpreter: a conditional is not
//! evaluated (`\iffalse\input{x}\fi` still names `x`), a file name built by a
//! macro is skipped, and a package that TeX Live supplies is known by name
//! only.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::lex::{lex_all, TokKind, Token};

/// Inputs are followed this deep; a cycle stops at the first repeat.
const MAX_DEPTH: usize = 32;

/// What loading a package also loads, for the packages mathfmt's spellings
/// can need. A local package's own `\RequirePackage` lines are read, so this
/// lists only distributed packages and classes.
const IMPLIES: &[(&str, &[&str])] = &[
    ("amssymb", &["amsfonts"]),
    ("mathtools", &["amsmath"]),
    ("unicode-math", &["amsmath", "fontspec"]),
    ("empheq", &["mathtools", "amsmath"]),
    ("physics", &["amsmath"]),
    ("breqn", &["amsmath"]),
    ("amsart", &["amsmath", "amsfonts", "amsthm"]),
    ("amsbook", &["amsmath", "amsfonts", "amsthm"]),
    ("amsproc", &["amsmath", "amsfonts", "amsthm"]),
];

/// The packages that loading `package` is known to load as well.
pub fn implied(package: &str) -> &'static [&'static str] {
    IMPLIES
        .iter()
        .find(|(name, _)| *name == package)
        .map(|(_, more)| *more)
        .unwrap_or(&[])
}

/// A root document and what it reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Project {
    /// The root, as it was named.
    pub root: PathBuf,
    /// The root's directory: `\input` paths resolve against it, as they do
    /// for TeX run from there.
    pub dir: PathBuf,
    /// The files to convert: the root, then every file its body inputs, in
    /// the order TeX reads them, each once.
    pub files: Vec<PathBuf>,
    /// Names the body inputs that no file beside the root answers
    /// (generated files, files TeX finds elsewhere).
    pub missing: Vec<String>,
    /// The document class.
    pub class: Option<String>,
    /// Every package the preamble loads, directly or through a local
    /// package or class, in order, each once.
    pub packages: Vec<String>,
    /// Local files read for their definitions: packages and classes found
    /// beside the root, and files the preamble inputs, in load order.
    pub definitions: Vec<PathBuf>,
    /// The engine a `% !TEX program = …` comment in the root names.
    pub program: Option<String>,
}

impl Project {
    /// Read a root document and everything it pulls in.
    pub fn read(root: &Path) -> std::io::Result<Project> {
        let text = std::fs::read_to_string(root)?;
        let dir = root.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut project = Project {
            root: root.to_path_buf(),
            dir,
            program: magic_program(&text),
            ..Project::default()
        };
        let mut reader = Reader {
            project: &mut project,
            seen: BTreeSet::new(),
        };
        reader.mark(root);
        reader.project.files.push(root.to_path_buf());
        reader.read(root, &text, Mode::Root, 0);
        Ok(project)
    }

    /// Does the preamble load this package, directly or through one that
    /// is known to load it?
    pub fn loads(&self, package: &str) -> bool {
        let mut stack: Vec<&str> = self.packages.iter().map(String::as_str).collect();
        stack.extend(self.class.as_deref());
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        while let Some(p) = stack.pop() {
            if p == package {
                return true;
            }
            if !seen.insert(p) {
                continue;
            }
            stack.extend(implied(p).iter().copied());
        }
        false
    }

    /// The engine the document is written for: the root's `% !TEX program`
    /// comment, else `lualatex` when the preamble loads `unicode-math` or
    /// `fontspec`, else `pdflatex`.
    pub fn engine(&self) -> &str {
        if let Some(p) = &self.program {
            return p;
        }
        if self.loads("unicode-math") || self.loads("fontspec") {
            "lualatex"
        } else {
            "pdflatex"
        }
    }
}

/// `% !TEX program = lualatex` (or `TS-program`) in the first lines.
fn magic_program(text: &str) -> Option<String> {
    for line in text.lines().take(30) {
        let Some(rest) = line.trim_start().strip_prefix('%') else {
            continue;
        };
        let lower = rest.to_ascii_lowercase();
        let Some(at) = lower.find("!tex") else {
            continue;
        };
        let after = &lower[at + 4..];
        let Some((key, value)) = after.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key == "program" || key == "ts-program" {
            let value = value.trim();
            if !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Some(value.to_string());
            }
        }
    }
    None
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The root: preamble until `\begin{document}`, body after.
    Root,
    /// A package, a class, or a file the preamble inputs.
    Preamble,
    /// A file the body inputs.
    Body,
}

struct Reader<'p> {
    project: &'p mut Project,
    seen: BTreeSet<PathBuf>,
}

impl Reader<'_> {
    /// Record a file as visited; false when it already was.
    fn mark(&mut self, path: &Path) -> bool {
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.seen.insert(key)
    }

    fn read(&mut self, path: &Path, text: &str, mode: Mode, depth: usize) {
        let tokens = lex_all(text);
        let mut preamble = mode != Mode::Body;
        let mut i = 0;
        while i < tokens.len() {
            let t = tokens[i];
            i += 1;
            if t.kind != TokKind::ControlWord {
                continue;
            }
            match t.name(text) {
                "documentclass" | "LoadClass" | "LoadClassWithOptions" if preamble => {
                    let Some((names, next)) = braced(&tokens, i, text, true) else {
                        continue;
                    };
                    i = next;
                    for name in list(&names) {
                        if mode == Mode::Root && self.project.class.is_none() {
                            self.project.class = Some(name.to_string());
                        } else {
                            self.package(name);
                        }
                        self.local(name, "cls", depth);
                    }
                }
                "usepackage" | "RequirePackage" | "RequirePackageWithOptions" if preamble => {
                    let Some((names, next)) = braced(&tokens, i, text, true) else {
                        continue;
                    };
                    i = next;
                    for name in list(&names) {
                        self.package(name);
                        self.local(name, "sty", depth);
                    }
                }
                name @ ("input" | "include" | "subfile" | "InputIfFileExists") => {
                    let Some((file, next)) = file_name(&tokens, i, text) else {
                        continue;
                    };
                    i = next;
                    let dir = self.project.dir.clone();
                    self.input(&dir, &file, name == "include", preamble, depth);
                }
                name @ ("import" | "subimport") => {
                    let Some((sub, next)) = braced(&tokens, i, text, false) else {
                        continue;
                    };
                    let Some((file, next)) = braced(&tokens, next, text, false) else {
                        continue;
                    };
                    i = next;
                    if !plain_name(&sub) {
                        continue;
                    }
                    let base = if name == "subimport" {
                        path.parent().map(Path::to_path_buf).unwrap_or_default()
                    } else {
                        self.project.dir.clone()
                    };
                    self.input(&base.join(sub.trim()), &file, false, preamble, depth);
                }
                "begin" | "end" if mode == Mode::Root => {
                    let begin = t.name(text) == "begin";
                    if let Some((env, next)) = braced(&tokens, i, text, false) {
                        if env.trim() == "document" {
                            i = next;
                            if begin {
                                preamble = false;
                            } else {
                                return;
                            }
                        }
                    }
                }
                "endinput" => return,
                _ => {}
            }
        }
    }

    fn package(&mut self, name: &str) {
        if !self.project.packages.iter().any(|p| p == name) {
            self.project.packages.push(name.to_string());
        }
    }

    /// A package or class that is a file beside the root is read for what
    /// it loads and defines.
    fn local(&mut self, name: &str, extension: &str, depth: usize) {
        if !plain_name(name) {
            return;
        }
        let path = self.project.dir.join(format!("{name}.{extension}"));
        self.definitions(&path, depth);
    }

    fn definitions(&mut self, path: &Path, depth: usize) {
        if depth >= MAX_DEPTH || !path.is_file() || !self.mark(path) {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        self.project.definitions.push(path.to_path_buf());
        self.read(path, &text, Mode::Preamble, depth + 1);
    }

    /// `\input{file}` seen in `base`: a definition file from the preamble, a
    /// file to convert from the body.
    fn input(&mut self, base: &Path, file: &str, include: bool, preamble: bool, depth: usize) {
        let file = file.trim();
        if !plain_name(file) {
            return;
        }
        let named = base.join(file);
        let with_tex = base.join(format!("{file}.tex"));
        let has_extension = Path::new(file).extension().is_some();
        let path = if include || !has_extension {
            if with_tex.is_file() || include {
                with_tex
            } else {
                named
            }
        } else if named.is_file() {
            named
        } else {
            with_tex
        };
        if !path.is_file() {
            if !preamble && !self.project.missing.iter().any(|m| m == file) {
                self.project.missing.push(file.to_string());
            }
            return;
        }
        if preamble {
            self.definitions(&path, depth);
            return;
        }
        if depth >= MAX_DEPTH || !self.mark(&path) {
            return;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        self.project.files.push(path.clone());
        self.read(&path, &text, Mode::Body, depth + 1);
    }
}

/// A name TeX reads literally: no macro, no parameter, not empty.
fn plain_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && !name.contains(['\\', '#', '{', '}', '%'])
}

fn list(names: &str) -> impl Iterator<Item = &str> {
    names.split(',').map(str::trim).filter(|n| plain_name(n))
}

fn skip_trivia(tokens: &[Token], mut i: usize) -> usize {
    while tokens.get(i).is_some_and(|t| t.kind == TokKind::Trivia) {
        i += 1;
    }
    i
}

/// The contents of the brace group at or after `i`, past an optional `[…]`
/// when `optional` is set, and the index after the group.
fn braced(tokens: &[Token], i: usize, text: &str, optional: bool) -> Option<(String, usize)> {
    let mut i = skip_trivia(tokens, i);
    if optional {
        while tokens
            .get(i)
            .is_some_and(|t| t.kind == TokKind::Char && t.text(text) == "[")
        {
            let mut depth = 0usize;
            loop {
                let t = tokens.get(i)?;
                i += 1;
                match t.kind {
                    TokKind::Open => depth += 1,
                    TokKind::Close => depth = depth.checked_sub(1)?,
                    TokKind::Char if depth == 0 && t.text(text) == "]" => break,
                    _ => {}
                }
            }
            i = skip_trivia(tokens, i);
        }
    }
    if tokens.get(i)?.kind != TokKind::Open {
        return None;
    }
    let start = tokens[i].span.end;
    let mut depth = 1usize;
    i += 1;
    loop {
        let t = tokens.get(i)?;
        i += 1;
        match t.kind {
            TokKind::Open => depth += 1,
            TokKind::Close => {
                depth -= 1;
                if depth == 0 {
                    return Some((text[start..t.span.start].to_string(), i));
                }
            }
            _ => {}
        }
    }
}

/// The file an `\input` names: a brace group, or TeX's own form, the
/// characters up to the next space.
fn file_name(tokens: &[Token], i: usize, text: &str) -> Option<(String, usize)> {
    let i = skip_trivia(tokens, i);
    if tokens.get(i)?.kind == TokKind::Open {
        return braced(tokens, i, text, false);
    }
    let start = tokens[i].span.start;
    let mut j = i;
    while tokens
        .get(j)
        .is_some_and(|t| matches!(t.kind, TokKind::Char | TokKind::Sub | TokKind::Sup))
    {
        j += 1;
    }
    if j == i {
        return None;
    }
    Some((text[start..tokens[j - 1].span.end].to_string(), j))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mathfmt-project-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        dir
    }

    fn names(dir: &Path, paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|p| p.strip_prefix(dir).unwrap().display().to_string())
            .collect()
    }

    #[test]
    fn follows_inputs_and_reads_local_packages() {
        let dir = tree(
            "follows",
            &[
                (
                    "main.tex",
                    "% !TEX program = pdflatex\n\\documentclass[11pt]{article}\n\
                     \\usepackage[utf8]{inputenc}\n\\usepackage{paper, mathtools}\n\
                     % \\usepackage{commented}\n\\input{defs}\n\
                     \\newcommand{\\loader}[1]{\\input{#1}}\n\
                     \\begin{document}\n\\input{sections/intro}\n\\include{sections/method}\n\
                     \\input sections/tail.tex \n% \\input{sections/never}\n\
                     \\input{generated/table}\n\\input{sections/intro}\n\
                     \\end{document}\n\\input{sections/after}\n",
                ),
                ("paper.sty", "\\RequirePackage{amssymb}\n\\RequirePackage{inner}\n\\endinput\n\\RequirePackage{never}\n"),
                ("inner.sty", "\\RequirePackage{bm}\n"),
                ("defs.tex", "\\newcommand{\\R}{\\mathbb{R}}\n"),
                ("sections/intro.tex", "Intro $x$ \\input{sections/deep}\n"),
                ("sections/deep.tex", "Deep. \\input{sections/intro}\n"),
                ("sections/method.tex", "Method.\n"),
                ("sections/tail.tex", "Tail.\n"),
                ("sections/after.tex", "Never read.\n"),
                ("sections/never.tex", "Never read.\n"),
            ],
        );
        let p = Project::read(&dir.join("main.tex")).unwrap();
        assert_eq!(
            names(&dir, &p.files),
            [
                "main.tex",
                "sections/intro.tex",
                "sections/deep.tex",
                "sections/method.tex",
                "sections/tail.tex"
            ]
        );
        assert_eq!(p.missing, ["generated/table"]);
        assert_eq!(p.class.as_deref(), Some("article"));
        assert_eq!(
            p.packages,
            ["inputenc", "paper", "amssymb", "inner", "bm", "mathtools"]
        );
        assert_eq!(
            names(&dir, &p.definitions),
            ["paper.sty", "inner.sty", "defs.tex"]
        );
        assert_eq!(p.program.as_deref(), Some("pdflatex"));
        assert!(p.loads("amsmath"), "mathtools loads amsmath");
        assert!(p.loads("amsfonts"), "amssymb loads amsfonts");
        assert!(p.loads("bm") && !p.loads("bbm") && !p.loads("never"));
        assert_eq!(p.engine(), "pdflatex");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_engine_follows_the_preamble() {
        let dir = tree(
            "engine",
            &[
                ("lua.tex", "\\documentclass{article}\n\\usepackage{unicode-math}\n\\begin{document}\\end{document}\n"),
                ("xe.tex", "%!TeX TS-program = xelatex\n\\documentclass{article}\n\\usepackage{fontspec}\n"),
                ("plain.tex", "\\documentclass{amsart}\n"),
                ("sub/main.tex", "\\documentclass{article}\\begin{document}\\subimport{parts/}{a}\\import{parts/}{b}\\end{document}"),
                ("sub/parts/a.tex", "\\subimport{deeper/}{c}"),
                ("sub/parts/b.tex", "b"),
                ("sub/parts/deeper/c.tex", "c"),
            ],
        );
        let lua = Project::read(&dir.join("lua.tex")).unwrap();
        assert_eq!(lua.engine(), "lualatex");
        assert!(lua.loads("unicode-math") && lua.loads("amsmath"));
        assert_eq!(
            Project::read(&dir.join("xe.tex")).unwrap().engine(),
            "xelatex"
        );
        let plain = Project::read(&dir.join("plain.tex")).unwrap();
        assert_eq!(plain.engine(), "pdflatex");
        assert!(plain.loads("amsmath"), "the amsart class loads amsmath");
        let sub = Project::read(&dir.join("sub/main.tex")).unwrap();
        assert_eq!(
            names(&dir, &sub.files),
            [
                "sub/main.tex",
                "sub/parts/a.tex",
                "sub/parts/deeper/c.tex",
                "sub/parts/b.tex"
            ]
        );
        assert!(Project::read(&dir.join("absent.tex")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
