//! `--verify`: prove a conversion preserved the typeset result.
//!
//! The original and the converted source are compiled with the same engine
//! and the same surroundings, both PDFs are rasterized with `pdftoppm`, and
//! the pages are compared pixel for pixel. Text extraction (`pdftotext`)
//! only supplies a hint about where a difference is: it cannot tell a bold
//! `x` from a plain one, and the pixels can.
//!
//! A file holding `\documentclass` is a document and is compiled as it is,
//! with the converted copies of every other input shadowing the originals;
//! so is its original, from a copy, so both compiles see the same files in
//! the same way. A file without one is a snippet and is wrapped in a
//! preamble: the built-in one for the engine, or `--preamble FILE`. The
//! built-in pdflatex preamble loads `unicode-math-input`, which reads Unicode
//! math under pdflatex, and declares every styled letter the snippet uses to
//! mean what mathfmt's `ascii` profile spells it as, because pdflatex has no
//! bold Greek under `\mathbf` and no blackboard digits under `\mathbb`, and
//! unicode-math-input follows pdflatex there. Symbols (`∈`, `≤`) are left to
//! unicode-math-input's own table: that is the independent check of
//! mathfmt's.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};
use rayon::prelude::*;

use mathfmt_core::{Atom, Engine, Profile, RenameMap};

#[derive(Clone, Debug)]
pub struct Options {
    /// The program to compile with: `pdflatex`, `lualatex`, `xelatex`, …
    pub engine: String,
    /// A preamble for snippets, replacing the built-in one.
    pub preamble: Option<PathBuf>,
    /// Rasterization resolution.
    pub dpi: u32,
    /// Keep the build directories here instead of a temporary directory.
    pub keep: Option<PathBuf>,
    /// Extra definition files named on the command line (`--macros`).
    pub macros: Vec<PathBuf>,
}

/// One input file, before and after.
pub struct Source<'a> {
    pub path: PathBuf,
    pub original: &'a str,
    pub converted: &'a str,
    pub engine: &'a Engine,
    pub profile: &'a Profile,
}

/// (left, top, right, bottom), in pixels of the rasterized page.
pub type Bbox = (u32, u32, u32, u32);

#[derive(Debug, PartialEq, Eq)]
pub struct PageDiff {
    /// 1-based.
    pub page: usize,
    pub pixels: usize,
    pub bbox: Bbox,
}

#[derive(Debug)]
pub enum Verdict {
    /// Nothing changed, so there is nothing to prove.
    Unchanged,
    /// Every page renders pixel for pixel the same.
    Identical { pages: usize },
    /// Some pages differ.
    Differs {
        pages: usize,
        diffs: Vec<PageDiff>,
        hint: Option<(String, String)>,
    },
    /// The two PDFs have different page counts.
    PageCount { original: usize, converted: usize },
    /// The converted source does not compile (the original does).
    Broken { excerpt: String },
}

impl Verdict {
    pub fn is_failure(&self) -> bool {
        !matches!(self, Verdict::Unchanged | Verdict::Identical { .. })
    }
}

pub struct Report {
    pub root: PathBuf,
    pub verdict: Verdict,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let root = self.root.display();
        match &self.verdict {
            Verdict::Unchanged => write!(f, "unchanged: {root}"),
            Verdict::Identical { pages } => {
                write!(
                    f,
                    "verified: {root} ({pages} page{} identical)",
                    plural(*pages)
                )
            }
            Verdict::Differs { pages, diffs, hint } => {
                write!(
                    f,
                    "differs: {root}: {} of {pages} page{}",
                    diffs.len(),
                    plural(*pages)
                )?;
                for d in diffs {
                    write!(
                        f,
                        "\n  page {}: {} pixel{} in the box x={}..{} y={}..{}",
                        d.page,
                        d.pixels,
                        plural(d.pixels),
                        d.bbox.0,
                        d.bbox.2,
                        d.bbox.1,
                        d.bbox.3
                    )?;
                }
                match hint {
                    Some((a, b)) => write!(f, "\n  text before: {a}\n  text after:  {b}"),
                    None => write!(f, "\n  the extracted text is the same; the difference is in glyph shapes or positions"),
                }
            }
            Verdict::PageCount {
                original,
                converted,
            } => write!(
                f,
                "differs: {root}: {original} page{} before, {converted} after",
                plural(*original)
            ),
            Verdict::Broken { excerpt } => {
                write!(f, "broken: {root}: the converted source does not compile")?;
                for line in excerpt.lines() {
                    write!(f, "\n  {line}")?;
                }
                Ok(())
            }
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Does this source carry its own `\documentclass` (outside comments)?
pub fn is_document(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim_start();
        !l.starts_with('%') && l.contains("\\documentclass")
    })
}

/// Verify every root (document, or each snippet when there is no document).
/// Roots are compiled in parallel.
pub fn run(sources: &[Source<'_>], opts: &Options) -> Result<Vec<Report>> {
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    check_tool(&opts.engine)?;
    check_tool("pdftoppm")?;
    let any_document = sources.iter().any(|s| is_document(s.original));
    let roots: Vec<usize> = (0..sources.len())
        .filter(|&i| !any_document || is_document(sources[i].original))
        .collect();
    let base = match &opts.keep {
        Some(dir) => {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            dir.clone()
        }
        None => {
            let dir = std::env::temp_dir().join(format!(
                "mathfmt-verify-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            dir
        }
    };
    let reports: Vec<Result<Report>> = roots
        .par_iter()
        .enumerate()
        .map(|(n, &i)| {
            let root = &sources[i];
            let dir = base.join(format!(
                "{n}-{}",
                root.path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ));
            verify_root(root, sources, &dir, opts).map(|verdict| Report {
                root: root.path.clone(),
                verdict,
            })
        })
        .collect();
    if opts.keep.is_none() {
        let _ = std::fs::remove_dir_all(&base);
    }
    reports.into_iter().collect()
}

fn check_tool(name: &str) -> Result<()> {
    match Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("--verify needs `{name}` on PATH")
        }
        Err(e) => Err(e).with_context(|| format!("running {name}")),
    }
}

fn verify_root(
    root: &Source<'_>,
    all: &[Source<'_>],
    dir: &Path,
    opts: &Options,
) -> Result<Verdict> {
    if root.original == root.converted
        && (!is_document(root.original) || all.iter().all(|s| s.original == s.converted))
    {
        return Ok(Verdict::Unchanged);
    }
    let document = is_document(root.original);
    let root_dir = root
        .path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let root_dir = std::fs::canonicalize(&root_dir)
        .with_context(|| format!("resolving {}", root_dir.display()))?;
    let root_name = root
        .path
        .file_name()
        .ok_or_else(|| anyhow!("{} has no file name", root.path.display()))?
        .to_os_string();

    // Two build trees, `a` for the original and `b` for the converted, each
    // with a copy of the sources at their relative paths and an output
    // directory for the compile. Both compiles run from their copy and see
    // the root's directory through TEXINPUTS, so unlisted files (style
    // files, figures, other inputs) are shared.
    let mut jobs = Vec::new();
    for (side, converted) in [("a", false), ("b", true)] {
        let src_dir = dir.join(side).join("src");
        let out_dir = dir.join(side).join("out");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&out_dir)?;
        let job_name = if document {
            for s in all {
                let text = if converted { s.converted } else { s.original };
                let rel = relative_to(&root_dir, &s.path)?;
                let target = src_dir.join(&rel);
                if let Some(p) = target.parent() {
                    std::fs::create_dir_all(p)?;
                }
                std::fs::write(&target, text)
                    .with_context(|| format!("writing {}", target.display()))?;
            }
            PathBuf::from(&root_name)
        } else {
            let text = if converted {
                root.converted
            } else {
                root.original
            };
            let wrapped = wrap_snippet(root, text, opts)?;
            std::fs::write(src_dir.join("snippet.tex"), wrapped)?;
            PathBuf::from("snippet.tex")
        };
        jobs.push((src_dir, out_dir, job_name));
    }

    let mut pdfs = Vec::new();
    for (i, (src_dir, out_dir, job)) in jobs.iter().enumerate() {
        match compile(&opts.engine, src_dir, out_dir, job, &root_dir, root)? {
            Ok(pdf) => pdfs.push(pdf),
            Err(excerpt) => {
                if i == 0 {
                    bail!(
                        "{}: the original does not compile with {}; nothing to compare against\n{}",
                        root.path.display(),
                        opts.engine,
                        excerpt
                    );
                }
                return Ok(Verdict::Broken { excerpt });
            }
        }
    }
    let a = rasterize(&pdfs[0], &dir.join("a"), opts.dpi)?;
    let b = rasterize(&pdfs[1], &dir.join("b"), opts.dpi)?;
    if a.len() != b.len() {
        return Ok(Verdict::PageCount {
            original: a.len(),
            converted: b.len(),
        });
    }
    let diffs: Vec<PageDiff> = a
        .iter()
        .zip(&b)
        .enumerate()
        .filter_map(|(i, (pa, pb))| {
            compare_pages(pa, pb).map(|(pixels, bbox)| PageDiff {
                page: i + 1,
                pixels,
                bbox,
            })
        })
        .collect();
    if diffs.is_empty() {
        return Ok(Verdict::Identical { pages: a.len() });
    }
    let hint = text_hint(&pdfs[0], &pdfs[1]);
    Ok(Verdict::Differs {
        pages: a.len(),
        diffs,
        hint,
    })
}

fn relative_to(root_dir: &Path, path: &Path) -> Result<PathBuf> {
    let abs =
        std::fs::canonicalize(path).with_context(|| format!("resolving {}", path.display()))?;
    abs.strip_prefix(root_dir)
        .map(Path::to_path_buf)
        .map_err(|_| {
            anyhow!(
                "{} is not under {}, the document's directory; --verify compiles from there",
                path.display(),
                root_dir.display()
            )
        })
}

/// Compile `job` in `src_dir`, writing to `out_dir`. `Ok(Err(excerpt))` is a
/// failed compile with the log's first error.
fn compile(
    engine: &str,
    src_dir: &Path,
    out_dir: &Path,
    job: &Path,
    root_dir: &Path,
    root: &Source<'_>,
) -> Result<Result<PathBuf, String>> {
    // The copy first, then the document's own directory, then the defaults.
    let mut texinputs = format!("{}:{}:", src_dir.display(), root_dir.display());
    if let Some(config_dir) = &root.engine.config.dir {
        texinputs = format!("{}{}:", texinputs, config_dir.display());
    }
    let output = Command::new(engine)
        .current_dir(src_dir)
        .env("TEXINPUTS", &texinputs)
        .env("SOURCE_DATE_EPOCH", "0")
        .arg("-interaction=nonstopmode")
        .arg("-halt-on-error")
        .arg(format!("-output-directory={}", out_dir.display()))
        .arg(job)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {engine}"))?;
    let stem = job.file_stem().unwrap_or_default();
    let pdf = out_dir.join(stem).with_extension("pdf");
    if output.status.success() && pdf.is_file() {
        return Ok(Ok(pdf));
    }
    let log_path = out_dir.join(stem).with_extension("log");
    let log = std::fs::read_to_string(&log_path)
        .unwrap_or_else(|_| String::from_utf8_lossy(&output.stdout).into_owned());
    Ok(Err(log_excerpt(&log)))
}

/// The first `!` error in a TeX log with its context, else the log's tail.
fn log_excerpt(log: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    if let Some(i) = lines.iter().position(|l| l.starts_with('!')) {
        return lines[i..lines.len().min(i + 6)].join("\n");
    }
    lines[lines.len().saturating_sub(10)..].join("\n")
}

/// Wrap a snippet in a document.
fn wrap_snippet(root: &Source<'_>, text: &str, opts: &Options) -> Result<String> {
    let mut out = String::new();
    match &opts.preamble {
        Some(p) => {
            out.push_str(
                &std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?,
            );
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        None => out.push_str(builtin_preamble(&opts.engine)?),
    }
    // Lines holding a preamble-only command move up whole; both sides are
    // treated alike, so the move cannot hide a difference.
    let mut body = String::new();
    for line in text.lines() {
        if is_preamble_line(line) {
            out.push_str(line);
            out.push('\n');
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    // The definitions mathfmt was told about are what the snippet assumes.
    let config_dir = root.engine.config.dir.clone();
    let macro_files = root
        .engine
        .config
        .macros
        .iter()
        .map(|m| match &config_dir {
            Some(d) => d.join(m),
            None => PathBuf::from(m),
        })
        .chain(opts.macros.iter().cloned());
    for m in macro_files {
        let abs = std::fs::canonicalize(&m).unwrap_or(m.clone());
        if abs.extension().is_some_and(|e| e == "sty") {
            let stem = abs.file_stem().unwrap_or_default().to_string_lossy();
            out.push_str(&format!(
                "\\makeatletter\\@ifpackageloaded{{{stem}}}{{}}{{\\usepackage{{{stem}}}}}\\makeatother\n"
            ));
        } else {
            out.push_str(&format!("\\input{{{}}}\n", abs.display()));
        }
    }
    if opts.preamble.is_none() && opts.engine == "pdflatex" {
        out.push_str(&styled_declarations(root));
    }
    out.push_str("\\begin{document}\n");
    out.push_str(&body);
    out.push_str("\\end{document}\n");
    Ok(out)
}

/// Commands LaTeX accepts only before `\begin{document}`.
const PREAMBLE_ONLY: &[&str] = &[
    "usepackage",
    "RequirePackage",
    "DeclareMathOperator",
    "DeclarePairedDelimiter",
    "newtheorem",
    "theoremstyle",
];

fn is_preamble_line(line: &str) -> bool {
    mathfmt_core::lex::lex_all(line).iter().any(|t| {
        t.kind == mathfmt_core::lex::TokKind::ControlWord && PREAMBLE_ONLY.contains(&t.name(line))
    })
}

fn builtin_preamble(engine: &str) -> Result<&'static str> {
    Ok(match engine {
        "pdflatex" | "latex" => {
            "\\documentclass{article}\n\
             \\usepackage{amsmath,amssymb,amsthm}\n\
             \\usepackage{bm}\n\
             \\usepackage{bbm}\n\
             \\usepackage{mathrsfs}\n\
             \\usepackage{mathtools}\n\
             \\usepackage{url}\n\
             \\usepackage{listings}\n\
             \\usepackage{unicode-math-input}\n"
        }
        "lualatex" | "xelatex" => {
            "\\documentclass{article}\n\
             \\usepackage{amsmath,amsthm}\n\
             \\usepackage{mathtools}\n\
             \\usepackage{url}\n\
             \\usepackage{listings}\n\
             \\usepackage{unicode-math}\n\
             \\providecommand{\\mathbbm}{\\mathbb}\n\
             \\providecommand{\\bm}{\\symbfit}\n"
        }
        other => bail!("no built-in preamble for `{other}`; give one with --preamble FILE"),
    })
}

/// `\umiDeclareMathChar` lines for every styled letter or digit the snippet
/// uses, before or after conversion, spelled as the `ascii` profile would.
fn styled_declarations(root: &Source<'_>) -> String {
    let engine = root.engine;
    let Ok(ascii) = engine.config.profile("ascii") else {
        return String::new();
    };
    let mut atoms: BTreeSet<Atom> = BTreeSet::new();
    for text in [root.original, root.converted] {
        let outcome = engine.convert(text, root.profile, &RenameMap::new());
        for occ in &outcome.occurrences {
            atoms.extend(occ.atoms.iter().copied());
        }
    }
    let mut out = String::new();
    for atom in atoms {
        let Atom::Styled { .. } = atom else { continue };
        let (Some(c), Some(spelling)) = (
            engine.table.char_of(&atom),
            mathfmt_core::print::spelling(&atom, &ascii, &engine.table),
        ) else {
            continue;
        };
        if !spelling.starts_with('\\') {
            continue;
        }
        if out.is_empty() {
            out.push_str(
                "% styled letters, as mathfmt's ascii profile spells them (see mathfmt table)\n",
            );
        }
        out.push_str(&format!("\\umiDeclareMathChar{{{c}}}{{{spelling}}}\n"));
    }
    out
}

/// A rasterized page: width, height, one gray byte per pixel.
struct Page {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

fn rasterize(pdf: &Path, dir: &Path, dpi: u32) -> Result<Vec<Page>> {
    let prefix = dir.join("page");
    let status = Command::new("pdftoppm")
        .arg("-gray")
        .arg("-r")
        .arg(dpi.to_string())
        .arg(pdf)
        .arg(&prefix)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .context("running pdftoppm")?;
    if !status.success() {
        bail!("pdftoppm failed on {}", pdf.display());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|e| e == "pgm")
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("page-"))
        })
        .collect();
    files.sort();
    files.iter().map(|p| read_pgm(p)).collect()
}

/// A binary PGM (`P5`) as pdftoppm writes it.
fn read_pgm(path: &Path) -> Result<Page> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut pos = 0usize;
    let mut fields: Vec<u32> = Vec::new();
    let mut magic = None;
    while fields.len() < 3 {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos < bytes.len() && bytes[pos] == b'#' {
            while pos < bytes.len() && bytes[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }
        let start = pos;
        while pos < bytes.len() && !bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let token = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
        if magic.is_none() {
            magic = Some(token.to_string());
            if token != "P5" {
                bail!("{}: not a binary PGM (got `{token}`)", path.display());
            }
        } else {
            fields.push(
                token
                    .parse()
                    .with_context(|| format!("{}: bad header", path.display()))?,
            );
        }
    }
    pos += 1; // the single whitespace after maxval
    let (width, height, maxval) = (fields[0], fields[1], fields[2]);
    if maxval > 255 {
        bail!("{}: 16-bit PGM is not supported", path.display());
    }
    let n = (width as usize) * (height as usize);
    if bytes.len() < pos + n {
        bail!("{}: truncated", path.display());
    }
    Ok(Page {
        width,
        height,
        data: bytes[pos..pos + n].to_vec(),
    })
}

/// `Some((differing pixels, bounding box))` when the pages differ.
fn compare_pages(a: &Page, b: &Page) -> Option<(usize, Bbox)> {
    if a.width != b.width || a.height != b.height {
        return Some((
            (a.width.max(b.width) as usize) * (a.height.max(b.height) as usize),
            (0, 0, a.width.max(b.width), a.height.max(b.height)),
        ));
    }
    let mut count = 0usize;
    let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for (i, (pa, pb)) in a.data.iter().zip(&b.data).enumerate() {
        if pa != pb {
            count += 1;
            let x = (i as u32) % a.width;
            let y = (i as u32) / a.width;
            left = left.min(x);
            right = right.max(x);
            top = top.min(y);
            bottom = bottom.max(y);
        }
    }
    (count > 0).then_some((count, (left, top, right, bottom)))
}

/// The first differing line of the two PDFs' extracted text, if `pdftotext`
/// is available and the texts differ.
fn text_hint(a: &Path, b: &Path) -> Option<(String, String)> {
    let extract = |pdf: &Path| -> Option<String> {
        let out = Command::new("pdftotext")
            .arg("-layout")
            .arg(pdf)
            .arg("-")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let (ta, tb) = (extract(a)?, extract(b)?);
    let la: Vec<&str> = ta.lines().collect();
    let lb: Vec<&str> = tb.lines().collect();
    for i in 0..la.len().max(lb.len()) {
        let x = la.get(i).copied().unwrap_or("");
        let y = lb.get(i).copied().unwrap_or("");
        if x != y {
            return Some((x.trim().to_string(), y.trim().to_string()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_are_recognized_outside_comments() {
        assert!(is_document("% x\n\\documentclass{article}\n"));
        assert!(!is_document("% \\documentclass{article}\n$x$\n"));
        assert!(!is_document("$x$"));
    }

    #[test]
    fn pgm_round_trip_and_compare() {
        let dir = std::env::temp_dir().join(format!("mathfmt-pgm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.pgm");
        std::fs::write(&p, b"P5\n# comment\n3 2\n255\n\x00\x10\x20\x30\x40\x50").unwrap();
        let a = read_pgm(&p).unwrap();
        assert_eq!((a.width, a.height), (3, 2));
        assert_eq!(a.data, vec![0x00, 0x10, 0x20, 0x30, 0x40, 0x50]);
        let mut b = Page {
            width: 3,
            height: 2,
            data: a.data.clone(),
        };
        assert_eq!(compare_pages(&a, &b), None);
        b.data[4] = 0xff;
        assert_eq!(compare_pages(&a, &b), Some((1, (1, 1, 1, 1))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn preamble_lines_are_recognized_by_their_commands() {
        assert!(is_preamble_line(
            r"\newcommand{\tr}{\mathrm{tr}}\DeclareMathOperator{\Var}{Var}"
        ));
        assert!(is_preamble_line(r"  \usepackage[utf8]{inputenc}"));
        assert!(!is_preamble_line(r"\newcommand{\R}{\mathbb{R}} $x$"));
        assert!(!is_preamble_line(r"% \usepackage{x}"));
    }

    #[test]
    fn log_excerpt_prefers_the_first_error() {
        let log = "line\n! Undefined control sequence.\nl.4 \\foo\n\nmore\nend";
        assert!(log_excerpt(log).starts_with("! Undefined"));
        assert_eq!(log_excerpt("a\nb"), "a\nb");
    }
}
