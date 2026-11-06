//! mathfmt: a filter that enforces math-notation conventions in LaTeX source.
//!
//! Reads LaTeX on stdin and writes it on stdout, or takes files. See
//! `mathfmt --help` and the design in `docs/superpowers/specs/`.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use rayon::prelude::*;

use mathfmt_core::lex::lex_all;
use mathfmt_core::{
    Atom, Config, Diagnostic, Engine, Level, LineIndex, Outcome, Profile, RegionKind, RenameMap,
};

#[derive(Parser)]
#[command(name = "mathfmt", version, about = "Rewrite math notation in LaTeX source to one profile", long_about = None)]
#[command(
    after_help = "Exit status: 0 clean; 1 --check found changes or --strict found warnings; 2 usage, config or I/O error.\n\
With no FILE, reads stdin and writes stdout. With FILEs and none of --write, --check, --diff or --emit, writes each converted file to stdout in turn."
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
}

#[derive(Args, Clone)]
struct RunArgs {
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
    /// Rewrite FILEs in place
    #[arg(short, long, conflicts_with_all = ["check", "emit"])]
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
    /// Diagnostics format
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Warnings make the exit status 1
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Table { profile, config }) => table(profile, config),
        Some(Command::Rename { from, to, run }) => convert(run, Some((from, to))),
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

fn load_config(explicit: Option<&Path>, near: &Path) -> Result<Config> {
    match explicit {
        Some(p) => Ok(Config::load(p)?),
        None => match Config::find(near) {
            Some(p) => Ok(Config::load(&p)?),
            None => Ok(Config::default()),
        },
    }
}

fn config_key(explicit: Option<&Path>, near: &Path) -> Option<PathBuf> {
    match explicit {
        Some(p) => Some(p.to_path_buf()),
        None => Config::find(near),
    }
}

fn build_engine(config: Config, macros: &[PathBuf]) -> Result<Engine> {
    let mut engine = Engine::new(config)?;
    for m in macros {
        engine.add_macro_file(m)?;
    }
    Ok(engine)
}

fn convert(args: RunArgs, rename: Option<(String, String)>) -> Result<ExitCode> {
    if let Some(j) = args.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(j.max(1))
            .build_global()
            .ok();
    }
    let cwd = std::env::current_dir().context("current directory")?;

    // Read every input first; a missing file is a usage error before any write.
    let inputs: Vec<Input> = if args.files.is_empty() {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .context("reading stdin")?;
        vec![Input { path: None, text }]
    } else {
        args.files
            .iter()
            .map(|p| {
                let text = std::fs::read_to_string(p)
                    .with_context(|| format!("reading {}", p.display()))?;
                Ok(Input {
                    path: Some(p.clone()),
                    text,
                })
            })
            .collect::<Result<Vec<_>>>()?
    };

    // One engine per distinct config file (the nearest mathfmt.toml per file).
    let mut groups: BTreeMap<Option<PathBuf>, Vec<usize>> = BTreeMap::new();
    for (i, input) in inputs.iter().enumerate() {
        let near = input
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(|d| {
                if d.as_os_str().is_empty() {
                    cwd.clone()
                } else {
                    cwd.join(d)
                }
            })
            .unwrap_or_else(|| cwd.clone());
        groups
            .entry(config_key(args.config.as_deref(), &near))
            .or_default()
            .push(i);
    }

    let mut converted: Vec<Option<Converted>> = (0..inputs.len()).map(|_| None).collect();
    for (key, indices) in groups {
        let near = key
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| cwd.clone());
        let config = load_config(args.config.as_deref(), &near)?;
        let engine = build_engine(config, &args.macros)?;
        let profile: Profile = engine.profile(args.profile.as_deref())?;
        let rename_map = match &rename {
            Some((from, to)) => {
                let mut m = RenameMap::new();
                m.insert(
                    engine.parse_atom(from, &profile)?,
                    engine.parse_atom(to, &profile)?,
                );
                m
            }
            None => RenameMap::new(),
        };
        if let Some(kind) = args.emit {
            for &i in &indices {
                emit(kind, &engine, &profile, &inputs[i])?;
            }
            continue;
        }
        let results: Vec<(usize, Converted)> = indices
            .par_iter()
            .map(|&i| {
                let input = inputs[i].clone();
                let outcome = engine.convert(&input.text, &profile, &rename_map);
                (i, Converted { input, outcome })
            })
            .collect();
        for (i, c) in results {
            converted[i] = Some(c);
        }
    }
    if args.emit.is_some() {
        return Ok(ExitCode::SUCCESS);
    }

    let mut stdout = io::stdout().lock();
    let mut any_changed = false;
    let mut any_warning = false;
    for c in converted.into_iter().flatten() {
        let Converted { input, outcome } = c;
        let index = LineIndex::new(&input.text);
        let label = input.label();
        let file = input.path.as_ref().map(|_| label.as_str());
        if !args.quiet {
            for d in &outcome.diagnostics {
                if d.level == Level::Info && !args.verbose {
                    continue;
                }
                let d = d.clone().locate(file, &index);
                any_warning |= d.level >= Level::Warning;
                report(&d, args.format)?;
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
                let path = input.path.as_ref().expect("--write needs files");
                write_atomically(path, &outcome.output)?;
            }
        } else if args.diff {
            if outcome.changed {
                write_diff(&mut stdout, &label, &input.text, &outcome.output)?;
            }
        } else {
            stdout.write_all(outcome.output.as_bytes())?;
        }
    }
    stdout.flush()?;
    let failed = (args.check && any_changed) || (args.strict && any_warning);
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
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

fn write_atomically(path: &Path, content: &str) -> Result<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("{} has no file name", path.display()))?;
    let tmp = dir.join(format!(".{}.mathfmt~", name.to_string_lossy()));
    std::fs::write(&tmp, content).with_context(|| format!("writing {}", tmp.display()))?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
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

fn emit(kind: Emit, engine: &Engine, profile: &Profile, input: &Input) -> Result<()> {
    let mut out = io::stdout().lock();
    let label = input.label();
    let index = LineIndex::new(&input.text);
    let src = input.text.as_str();
    match kind {
        Emit::Symbols => {
            for s in engine.inventory(src, profile) {
                let v = serde_json::json!({
                    "file": label,
                    "atom": s.atom.key(),
                    "unicode": s.unicode.map(|c| c.to_string()),
                    "name": s.name,
                    "count": s.count,
                    "spellings": s.spellings,
                });
                writeln!(out, "{v}")?;
            }
        }
        Emit::Atoms => {
            let outcome = engine.convert(src, profile, &RenameMap::new());
            for o in &outcome.occurrences {
                let v = serde_json::json!({
                    "file": label,
                    "start": position(&index, o.span.start),
                    "end": position(&index, o.span.end),
                    "source": o.span.text(src),
                    "kind": format!("{:?}", o.kind).to_lowercase(),
                    "atoms": o.atoms.iter().map(|a| atom_json(engine, a)).collect::<Vec<_>>(),
                });
                writeln!(out, "{v}")?;
            }
        }
        Emit::Regions | Emit::Tokens => {
            let doc = mathfmt_core::scan(
                src,
                &engine.signatures,
                &engine.scan_options,
                engine.macros.clone(),
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
                    let v = serde_json::json!({
                        "file": label,
                        "kind": region_kind,
                        "start": position(&index, body.start),
                        "end": position(&index, body.end),
                        "text": body.text(src),
                    });
                    writeln!(out, "{v}")?;
                } else {
                    for t in lex_all(body.text(src)) {
                        let start = body.start + t.span.start;
                        let end = body.start + t.span.end;
                        let v = serde_json::json!({
                            "file": label,
                            "region": region_kind,
                            "kind": t.kind,
                            "text": &src[start..end],
                            "start": position(&index, start),
                        });
                        writeln!(out, "{v}")?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn table(profile: Option<String>, config: Option<PathBuf>) -> Result<ExitCode> {
    let cwd = std::env::current_dir().context("current directory")?;
    let config = load_config(config.as_deref(), &cwd)?;
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
            info.map(|i| i
                .aliases
                .iter()
                .map(|a| format!("\\{a}"))
                .collect::<Vec<_>>()
                .join(","))
                .unwrap_or_default(),
        )?;
    }
    Ok(ExitCode::SUCCESS)
}
