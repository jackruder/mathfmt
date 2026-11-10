//! mathfmt: a filter that enforces math-notation conventions in LaTeX source.
//!
//! Reads LaTeX on stdin and writes it on stdout, or takes files. See
//! `mathfmt --help` and the design in `docs/superpowers/specs/`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use rayon::prelude::*;

use mathfmt_core::lex::lex_all;
use mathfmt_core::{
    Atom, Config, Diagnostic, Engine, Level, LineIndex, Mechanism, Outcome, Profile, RegionKind,
    RenameMap,
};

mod verify;

#[derive(Parser)]
#[command(name = "mathfmt", version, about = "Rewrite math notation in LaTeX source to one profile", long_about = None)]
#[command(
    after_help = "Exit status: 0 clean; 1 --check found changes, --strict found warnings or --verify found a difference; 2 usage, config or I/O error, or an original that --verify could not compile.\n\
With no FILE, reads stdin and writes stdout. With FILEs and none of --write, --check, --diff, --emit or --verify, writes each converted file to stdout in turn.\n\
--project takes the files from a root document: every file it reads with \\input or \\include. The root is the FILE, else --root, else `root` in mathfmt.toml. With a root, named either way, the definitions of its preamble and of the local packages it loads are in force for every file.\n\
--verify compiles the original and the converted source with the same engine and compares the rendered pages pixel for pixel. A FILE holding \\documentclass is a document, compiled as it is with the converted copies of the other FILEs in place; a FILE no document reads is a snippet, wrapped in a preamble and verified on its own. Needs the engine and pdftoppm on PATH."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    run: RunArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Replace one symbol by another everywhere, whatever its spelling
    Rename {
        /// The symbol to replace, as any spelling: `𝔉`, `\mathfrak{F}`, `\F`
        #[arg(long)]
        from: String,
        /// Its replacement, as any spelling
        #[arg(long)]
        to: String,
        #[command(flatten)]
        run: RunArgs,
    },
    /// Print the effective symbol table as TSV
    Table {
        #[arg(short, long)]
        profile: Option<String>,
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Print the preamble lines the notation needs: the packages its
    /// commands come from and a meaning for each Unicode character
    Declarations {
        #[command(flatten)]
        select: Select,
        /// How the document reads Unicode math (default: `unicode_input`
        /// in the config, else unicode-math when the preamble loads it or
        /// the engine is not pdflatex, else unicode-math-input)
        #[arg(long = "with", value_enum, value_name = "MECHANISM")]
        with: Option<With>,
    },
    /// Print what project mode reads from a root document
    Project {
        /// The root document (default: `root` in mathfmt.toml)
        root: Option<PathBuf>,
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
}

/// Which files, read under which config, profile and definitions.
#[derive(Args, Clone)]
struct Select {
    /// LaTeX files to convert; none means stdin to stdout
    files: Vec<PathBuf>,
    /// Profile from the config (default: the config's `profile`)
    #[arg(short, long)]
    profile: Option<String>,
    /// Config file (default: the nearest mathfmt.toml above each file)
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// An extra file to read macro definitions from (repeatable)
    #[arg(long, value_name = "FILE", action = clap::ArgAction::Append)]
    macros: Vec<PathBuf>,
    /// FILEs are root documents: take every file each one inputs. With no
    /// FILE, the root is --root, else `root` in mathfmt.toml
    #[arg(long)]
    project: bool,
    /// The root document of the FILEs: its preamble's packages and
    /// definitions apply to them (default: `root` in mathfmt.toml; a FILE
    /// that is itself a document is its own root)
    #[arg(long, value_name = "FILE")]
    root: Option<PathBuf>,
    /// The TeX engine: what --verify compiles with, and what the profile's
    /// commands and `declarations` assume (default: `engine` in the config,
    /// else what the root's preamble says, else pdflatex)
    #[arg(long, value_name = "PROGRAM")]
    engine: Option<String>,
}

#[derive(Args, Clone)]
struct RunArgs {
    #[command(flatten)]
    select: Select,
    /// Rewrite FILEs in place (through a symlink to its target)
    #[arg(short, long, conflicts_with_all = ["check", "diff", "emit"])]
    write: bool,
    /// Exit 1 if any file would change, and name it; write nothing
    #[arg(long)]
    check: bool,
    /// Print a unified diff per changed file instead of its content
    #[arg(long)]
    diff: bool,
    /// Print an intermediate stage as JSON lines instead of converting
    #[arg(long, value_enum, value_name = "KIND")]
    emit: Option<Emit>,
    /// Compile before and after and compare the rendered pages; print a
    /// verdict per document or snippet, exit 1 on a difference
    #[arg(long, conflicts_with_all = ["write", "check", "diff", "emit"])]
    verify: bool,
    /// Under --verify, add what `mathfmt declarations` prints to a
    /// document's preamble, on both sides
    #[arg(long, requires = "verify")]
    declare: bool,
    /// Preamble for snippets under --verify (default: built in, per engine)
    #[arg(long, value_name = "FILE", requires = "verify")]
    preamble: Option<PathBuf>,
    /// Resolution of the page comparison under --verify, 50 to 2400; a
    /// coarse raster hides small differences
    #[arg(
        long,
        value_name = "N",
        default_value_t = 100,
        requires = "verify",
        value_parser = clap::value_parser!(u32).range(50..=2400)
    )]
    dpi: u32,
    /// Keep the --verify build directories here
    #[arg(long, value_name = "DIR", requires = "verify")]
    keep: Option<PathBuf>,
    /// Diagnostics format
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Warnings make the exit status 1 (counted even under --quiet)
    #[arg(long)]
    strict: bool,
    /// Files to convert in parallel (default: available cores)
    #[arg(short, long)]
    jobs: Option<usize>,
    /// No diagnostics on stderr
    #[arg(short, long, conflicts_with = "verbose")]
    quiet: bool,
    /// Also show informational diagnostics
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum Emit {
    Regions,
    Tokens,
    Atoms,
    Symbols,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum Format {
    Text,
    Json,
}

/// How the document reads Unicode math characters.
#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum With {
    /// pdflatex with the unicode-math-input package
    UnicodeMathInput,
    /// pdflatex with the kernel's \DeclareUnicodeCharacter alone
    Kernel,
    /// lualatex or xelatex with unicode-math
    UnicodeMath,
}

impl From<With> for Mechanism {
    fn from(w: With) -> Mechanism {
        match w {
            With::UnicodeMathInput => Mechanism::UnicodeMathInput,
            With::Kernel => Mechanism::Kernel,
            With::UnicodeMath => Mechanism::UnicodeMath,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Options given before a subcommand belong to the top level and would
    // be dropped on the floor: `mathfmt -p ascii declarations` is not
    // `mathfmt declarations -p ascii`.
    if cli.command.is_some() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if let Some(at) = args
            .iter()
            .position(|a| matches!(a.as_str(), "declarations" | "project" | "table" | "rename"))
        {
            if at > 0 {
                eprintln!(
                    "mathfmt: `{}` before the subcommand `{}` would be ignored; put options after it",
                    args[..at].join(" "),
                    args[at]
                );
                return ExitCode::from(2);
            }
        }
    }
    let result = match cli.command {
        Some(Command::Table { profile, config }) => table(profile, config),
        Some(Command::Rename { from, to, run }) => convert(run, Some((from, to))),
        Some(Command::Declarations { select, with }) => declarations(select, with),
        Some(Command::Project { root, config }) => project(root, config),
        None => convert(cli.run, None),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("mathfmt: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// One input: a path, or stdin.
#[derive(Clone)]
struct Input {
    path: Option<PathBuf>,
    text: String,
}

impl Input {
    fn label(&self) -> String {
        match &self.path {
            Some(p) => p.display().to_string(),
            None => "<stdin>".to_string(),
        }
    }
}

struct Converted {
    input: Input,
    outcome: Outcome,
}

/// The config that governs a file: the one named on the command line, else
/// the nearest `mathfmt.toml` at or above `near`, else the defaults. The key
/// identifies the file (None for the defaults) so inputs can share an engine.
fn locate_config(explicit: Option<&Path>, near: &Path) -> Result<(Option<PathBuf>, Config)> {
    let path = match explicit {
        Some(p) => Some(p.to_path_buf()),
        None => Config::find(near),
    };
    let config = match &path {
        Some(p) => Config::load(p)?,
        None => Config::default(),
    };
    Ok((path, config))
}

/// The inputs that share a config and a root, with the engine built for them.
struct Group {
    engine: Engine,
    profile: Profile,
    /// Indices into the inputs.
    indices: Vec<usize>,
}

/// What a command line selects: the inputs, grouped by the engine that
/// converts them.
struct Loaded {
    inputs: Vec<Input>,
    groups: Vec<Group>,
    /// The group of each input.
    group_of: Vec<usize>,
}

struct Loader<'a> {
    select: &'a Select,
    cwd: PathBuf,
    /// (config file, root document) → group.
    index: BTreeMap<(Option<PathBuf>, Option<PathBuf>), usize>,
    groups: Vec<Group>,
}

impl Loader<'_> {
    /// A path under the current directory, as the user would name it.
    fn pretty(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.cwd)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.to_path_buf())
    }

    /// The directory a file's config is looked up from.
    fn near(&self, file: &Path) -> PathBuf {
        match file.parent() {
            Some(d) if !d.as_os_str().is_empty() => self.cwd.join(d),
            _ => self.cwd.clone(),
        }
    }

    /// The group for files near `near`, under `root` when the caller names
    /// one, else `--root`, else the config's `root`, else none.
    fn group(
        &mut self,
        near: &Path,
        root: Option<&Path>,
        fallback: Option<&Path>,
    ) -> Result<usize> {
        let (config_path, config) = locate_config(self.select.config.as_deref(), near)?;
        let root = root
            .map(Path::to_path_buf)
            .or_else(|| self.select.root.clone())
            .or_else(|| config.root_path().map(|p| self.pretty(&p)))
            .or_else(|| fallback.map(Path::to_path_buf));
        let key = (config_path, root.clone());
        if let Some(&g) = self.index.get(&key) {
            return Ok(g);
        }
        let mut engine = Engine::with_root(config, root.as_deref())?;
        // The engine named on the command line is the config's `engine`
        // for this run, so the profile, the declarations and --verify
        // agree on it.
        if self.select.engine.is_some() {
            engine.config.engine = self.select.engine.clone();
        }
        for m in &self.select.macros {
            engine.add_macro_file(m)?;
        }
        let profile = engine.profile(self.select.profile.as_deref())?;
        self.groups.push(Group {
            engine,
            profile,
            indices: Vec::new(),
        });
        self.index.insert(key, self.groups.len() - 1);
        Ok(self.groups.len() - 1)
    }
}

fn read_input(path: &Path) -> Result<Input> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(Input {
        path: Some(path.to_path_buf()),
        text,
    })
}

/// Read every input and build an engine per config and root. Everything is
/// read before anything is written; a missing file is an error here.
fn load(select: &Select) -> Result<Loaded> {
    let cwd = std::env::current_dir().context("current directory")?;
    let mut loader = Loader {
        select,
        cwd,
        index: BTreeMap::new(),
        groups: Vec::new(),
    };
    let mut inputs: Vec<Input> = Vec::new();
    let mut group_of: Vec<usize> = Vec::new();
    if select.project {
        let roots: Vec<PathBuf> = if !select.files.is_empty() {
            select.files.clone()
        } else if let Some(r) = &select.root {
            vec![r.clone()]
        } else {
            let (_, config) = locate_config(select.config.as_deref(), &loader.cwd)?;
            match config.root_path() {
                Some(p) => vec![loader.pretty(&p)],
                None => bail!(
                    "--project needs a root document: name it (`mathfmt --project main.tex`) or set `root` in mathfmt.toml"
                ),
            }
        };
        // A file two roots share is converted once, with the first.
        let mut taken: BTreeSet<PathBuf> = BTreeSet::new();
        for root in roots {
            let near = loader.near(&root);
            let g = loader.group(&near, Some(&root), None)?;
            let files = loader.groups[g]
                .engine
                .project
                .as_ref()
                .map(|p| p.files.clone())
                .unwrap_or_default();
            for f in files {
                let key = std::fs::canonicalize(&f).unwrap_or_else(|_| f.clone());
                if !taken.insert(key) {
                    continue;
                }
                loader.groups[g].indices.push(inputs.len());
                group_of.push(g);
                inputs.push(read_input(&f)?);
            }
        }
    } else if select.files.is_empty() {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .context("reading stdin")?;
        let near = loader.cwd.clone();
        let g = loader.group(&near, None, None)?;
        loader.groups[g].indices.push(0);
        group_of.push(g);
        inputs.push(Input { path: None, text });
    } else {
        for p in &select.files {
            inputs.push(read_input(p)?);
        }
        for (i, p) in select.files.iter().enumerate() {
            let near = loader.near(p);
            // With no root in force (no --root, none in the config), a
            // document named on its own is its own root, and a subfile's
            // is the main document it names: their preamble decides the
            // engine and the commands, as under --project.
            let text = &inputs[i].text;
            let fallback = match mathfmt_core::project::subfile_main(text) {
                Some(main) => {
                    let base = p.parent().map(Path::to_path_buf).unwrap_or_default();
                    let main = base.join(&main);
                    Some(if main.extension().is_some() {
                        main
                    } else {
                        main.with_extension("tex")
                    })
                }
                None => mathfmt_core::project::is_document(text).then(|| p.clone()),
            };
            let g = loader.group(&near, None, fallback.as_deref())?;
            loader.groups[g].indices.push(i);
            group_of.push(g);
        }
    }
    Ok(Loaded {
        inputs,
        groups: loader.groups,
        group_of,
    })
}

fn convert(args: RunArgs, rename: Option<(String, String)>) -> Result<ExitCode> {
    if let Some(j) = args.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(j.max(1))
            .build_global()
            .ok();
    }
    if args.write && args.select.files.is_empty() && !args.select.project {
        bail!("--write needs FILEs or --project; stdin is never written back");
    }

    let Loaded {
        inputs,
        groups,
        group_of,
    } = load(&args.select)?;

    let mut converted: Vec<Option<Converted>> = (0..inputs.len()).map(|_| None).collect();
    let mut emitted: Vec<Option<String>> = (0..inputs.len()).map(|_| None).collect();
    for group in &groups {
        let Group {
            engine,
            profile,
            indices,
        } = group;
        let rename_map = match &rename {
            Some((from, to)) => {
                let mut m = RenameMap::new();
                m.insert(
                    engine.parse_atom(from, profile)?,
                    engine.parse_atom(to, profile)?,
                );
                m
            }
            None => RenameMap::new(),
        };
        if let Some(kind) = args.emit {
            for &i in indices {
                emitted[i] = Some(emit(kind, engine, profile, &inputs[i])?);
            }
            continue;
        }
        let results: Vec<(usize, Converted)> = indices
            .par_iter()
            .map(|&i| {
                let input = inputs[i].clone();
                let outcome = engine.convert(&input.text, profile, &rename_map);
                (i, Converted { input, outcome })
            })
            .collect();
        for (i, c) in results {
            converted[i] = Some(c);
        }
    }

    let mut stdout = io::stdout().lock();
    if args.emit.is_some() {
        for text in emitted.into_iter().flatten() {
            stdout.write_all(text.as_bytes())?;
        }
        stdout.flush()?;
        return Ok(ExitCode::SUCCESS);
    }

    let mut any_changed = false;
    let mut any_warning = false;
    for c in converted.iter().flatten() {
        let Converted { input, outcome } = c;
        let index = LineIndex::new(&input.text);
        let label = input.label();
        let file = input.path.as_ref().map(|_| label.as_str());
        any_warning |= outcome
            .diagnostics
            .iter()
            .any(|d| d.level >= Level::Warning);
        if !args.quiet {
            for d in &outcome.diagnostics {
                if d.level == Level::Info && !args.verbose {
                    continue;
                }
                report(&d.clone().locate(file, &index), args.format)?;
            }
        }
        any_changed |= outcome.changed;
        if args.check {
            if outcome.changed {
                writeln!(stdout, "would change: {label}")?;
                if args.diff {
                    write_diff(&mut stdout, &label, &input.text, &outcome.output)?;
                }
            }
        } else if args.write {
            if outcome.changed {
                let path = input.path.as_ref().expect("--write was given files");
                write_atomically(path, &outcome.output)?;
            }
        } else if args.diff {
            if outcome.changed {
                write_diff(&mut stdout, &label, &input.text, &outcome.output)?;
            }
        } else if !args.verify {
            stdout.write_all(outcome.output.as_bytes())?;
        }
    }
    stdout.flush()?;
    let mut any_difference = false;
    let mut any_error = false;
    if args.verify {
        // What `mathfmt declarations` would print for each group, when the
        // run asks for it in the preamble.
        let declared: Vec<Option<String>> = groups
            .iter()
            .map(|g| {
                args.declare.then(|| {
                    let texts: Vec<&str> =
                        g.indices.iter().map(|&i| inputs[i].text.as_str()).collect();
                    g.engine.declarations(&texts, &g.profile, None).render()
                })
            })
            .collect();
        let sources: Vec<verify::Source<'_>> = converted
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.as_ref().map(|c| (i, c)))
            .map(|(i, c)| {
                let group = &groups[group_of[i]];
                Ok(verify::Source {
                    path: c
                        .input
                        .path
                        .clone()
                        .ok_or_else(|| anyhow!("--verify needs FILEs, not stdin"))?,
                    original: &c.input.text,
                    converted: &c.outcome.output,
                    engine: &group.engine,
                    profile: &group.profile,
                    program: group.engine.program(),
                    is_root: !args.select.project
                        || group
                            .engine
                            .project
                            .as_ref()
                            .and_then(|p| p.files.first())
                            .is_some_and(|f| f == &c.input.path.clone().unwrap_or_default()),
                    declare: declared[group_of[i]].as_deref(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let options = verify::Options {
            preamble: args.preamble.clone(),
            dpi: args.dpi,
            keep: args.keep.clone(),
            macros: args.select.macros.clone(),
        };
        for report in verify::run(&sources, &options)? {
            any_difference |= report.verdict.is_failure();
            any_error |= report.verdict.is_error();
            writeln!(stdout, "{report}")?;
        }
        stdout.flush()?;
    }
    // Every verdict is printed first; one that could not be reached (an
    // original that does not compile) is an error, not a difference.
    if any_error {
        return Ok(ExitCode::from(2));
    }
    let failed = (args.check && any_changed) || (args.strict && any_warning) || any_difference;
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// `mathfmt declarations`: the preamble lines each group's notation needs.
fn declarations(select: Select, with: Option<With>) -> Result<ExitCode> {
    let loaded = load(&select)?;
    let mut out = io::stdout().lock();
    for group in &loaded.groups {
        let texts: Vec<&str> = group
            .indices
            .iter()
            .map(|&i| loaded.inputs[i].text.as_str())
            .collect();
        let d = group
            .engine
            .declarations(&texts, &group.profile, with.map(Mechanism::from));
        let what = match &group.engine.project {
            Some(p) => p.root.display().to_string(),
            None => match texts.len() {
                1 => loaded.inputs[group.indices[0]].label(),
                n => format!("{n} files"),
            },
        };
        writeln!(
            out,
            "% mathfmt declarations: {what}, profile {}, {}",
            group.profile.name,
            d.mechanism.as_str()
        )?;
        let body = d.render();
        if body.is_empty() {
            writeln!(out, "% nothing to add")?;
        } else {
            out.write_all(body.as_bytes())?;
        }
    }
    out.flush()?;
    Ok(ExitCode::SUCCESS)
}

/// `mathfmt project`: what project mode reads from a root, as TSV.
fn project(root: Option<PathBuf>, config: Option<PathBuf>) -> Result<ExitCode> {
    let select = Select {
        files: Vec::new(),
        profile: None,
        config,
        macros: Vec::new(),
        project: true,
        root,
        engine: None,
    };
    let loaded = load(&select)?;
    let mut out = io::stdout().lock();
    for group in &loaded.groups {
        let Some(p) = &group.engine.project else {
            continue;
        };
        writeln!(out, "root\t{}", p.root.display())?;
        writeln!(out, "engine\t{}", group.engine.program())?;
        writeln!(out, "unicode-input\t{}", group.engine.mechanism().as_str())?;
        if let Some(class) = &p.class {
            writeln!(out, "class\t{class}")?;
        }
        for f in &p.files {
            writeln!(out, "file\t{}", f.display())?;
        }
        for m in &p.missing {
            writeln!(out, "missing\t{m}")?;
        }
        for d in &p.definitions {
            writeln!(out, "definitions\t{}", d.display())?;
        }
        for package in &p.packages {
            writeln!(out, "package\t{package}")?;
        }
        writeln!(out, "macros\t{}", group.engine.macros.len())?;
    }
    out.flush()?;
    Ok(ExitCode::SUCCESS)
}

fn report(d: &Diagnostic, format: Format) -> Result<()> {
    match format {
        Format::Text => eprintln!("{d}"),
        Format::Json => eprintln!("{}", serde_json::to_string(d)?),
    }
    Ok(())
}

fn write_diff(out: &mut impl Write, label: &str, old: &str, new: &str) -> Result<()> {
    let diff = similar::TextDiff::from_lines(old, new);
    write!(
        out,
        "{}",
        diff.unified_diff()
            .context_radius(3)
            .header(&format!("a/{label}"), &format!("b/{label}"))
    )?;
    Ok(())
}

/// Replace the file's contents: write a sibling temp file, copy the
/// permissions, rename over the target. A symlink is followed, so the real
/// file changes and the link stays a link.
fn write_atomically(path: &Path, content: &str) -> Result<()> {
    let target =
        std::fs::canonicalize(path).with_context(|| format!("resolving {}", path.display()))?;
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .ok_or_else(|| anyhow!("{} has no file name", target.display()))?;
    let tmp = dir.join(format!(".{}.mathfmt~", name.to_string_lossy()));
    std::fs::write(&tmp, content).with_context(|| format!("writing {}", tmp.display()))?;
    if let Ok(meta) = std::fs::metadata(&target) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, &target).with_context(|| format!("replacing {}", target.display()))?;
    Ok(())
}

fn position(index: &LineIndex, offset: usize) -> serde_json::Value {
    let (line, column) = index.line_col(offset);
    serde_json::json!({ "line": line, "column": column, "offset": offset })
}

fn atom_json(engine: &Engine, atom: &Atom) -> serde_json::Value {
    let unicode = engine.table.char_of(atom).map(|c| c.to_string());
    let name = engine
        .table
        .char_of(atom)
        .and_then(|c| engine.table.info(c))
        .and_then(|i| i.name.clone());
    serde_json::json!({ "atom": atom.key(), "unicode": unicode, "name": name })
}

/// One input's `--emit` output, as JSON lines.
fn emit(kind: Emit, engine: &Engine, profile: &Profile, input: &Input) -> Result<String> {
    let mut out = String::new();
    let label = input.label();
    let index = LineIndex::new(&input.text);
    let src = input.text.as_str();
    let mut line = |v: serde_json::Value| {
        out.push_str(&v.to_string());
        out.push('\n');
    };
    match kind {
        Emit::Symbols => {
            for s in engine.inventory(src, profile) {
                line(serde_json::json!({
                    "file": label,
                    "atom": s.atom.key(),
                    "unicode": s.unicode.map(|c| c.to_string()),
                    "name": s.name,
                    "count": s.count,
                    "spellings": s.spellings,
                }));
            }
            for n in engine.notation_inventory(src, profile) {
                line(serde_json::json!({
                    "file": label,
                    "notation": n.name,
                    "count": n.count,
                    "forms": n.forms,
                }));
            }
        }
        Emit::Atoms => {
            let outcome = engine.convert(src, profile, &RenameMap::new());
            for o in &outcome.occurrences {
                line(serde_json::json!({
                    "file": label,
                    "start": position(&index, o.span.start),
                    "end": position(&index, o.span.end),
                    "source": o.span.text(src),
                    "kind": format!("{:?}", o.kind).to_lowercase(),
                    "atoms": o.atoms.iter().map(|a| atom_json(engine, a)).collect::<Vec<_>>(),
                }));
            }
            for n in &outcome.notations {
                line(serde_json::json!({
                    "file": label,
                    "start": position(&index, n.span.start),
                    "end": position(&index, n.span.end),
                    "source": n.span.text(src),
                    "kind": "notation",
                    "notation": n.name,
                    "form": n.form,
                    "holes": n.holes.iter().map(|h| h.text(src)).collect::<Vec<_>>(),
                    "rewritten": n.rewritten,
                }));
            }
        }
        Emit::Regions | Emit::Tokens => {
            let doc = mathfmt_core::scan(
                src,
                &engine.signatures,
                &engine.scan_options,
                &engine.macros,
            );
            for r in &doc.regions {
                let region_kind = match &r.kind {
                    RegionKind::Inline => "inline".to_string(),
                    RegionKind::Display => "display".to_string(),
                    RegionKind::Env(e) => format!("env:{e}"),
                    RegionKind::Ensuremath => "ensuremath".to_string(),
                };
                let body = r.body();
                if kind == Emit::Regions {
                    line(serde_json::json!({
                        "file": label,
                        "kind": region_kind,
                        "start": position(&index, body.start),
                        "end": position(&index, body.end),
                        "text": body.text(src),
                    }));
                } else {
                    for t in lex_all(body.text(src)) {
                        let start = body.start + t.span.start;
                        let end = body.start + t.span.end;
                        line(serde_json::json!({
                            "file": label,
                            "region": region_kind,
                            "kind": t.kind,
                            "text": &src[start..end],
                            "start": position(&index, start),
                        }));
                    }
                }
            }
        }
    }
    Ok(out)
}

fn table(profile: Option<String>, config: Option<PathBuf>) -> Result<ExitCode> {
    let cwd = std::env::current_dir().context("current directory")?;
    let (_, config) = locate_config(config.as_deref(), &cwd)?;
    let engine = Engine::new(config)?;
    let profile = engine.profile(profile.as_deref())?;
    let mut out = io::stdout().lock();
    writeln!(
        out,
        "codepoint\tatom\tunicode\tlatex\tprofile:{}\tname\taliases",
        profile.name
    )?;
    for (c, atom) in engine.table.all_atoms() {
        let info = engine.table.info(c);
        writeln!(
            out,
            "{:04X}\t{}\t{}\t{}\t{}\t{}\t{}",
            c as u32,
            atom.key(),
            c,
            engine.table.latex_of(&atom).unwrap_or(""),
            mathfmt_core::print::spelling(&atom, &profile, &engine.table).unwrap_or_default(),
            info.and_then(|i| i.name.as_deref()).unwrap_or(""),
            info.map(|i| {
                i.aliases
                    .iter()
                    .map(|a| format!("\\{a}"))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default(),
        )?;
    }
    Ok(ExitCode::SUCCESS)
}
