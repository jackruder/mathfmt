//! The golden cases under `tests/fixtures/<case>/` (design §14).
//!
//! Each case holds `input.tex` and `expected.tex`, and optionally
//! `mathfmt.toml`, `args` (one CLI argument per line: `--profile NAME`,
//! `--macros FILE`, `rename --from X --to Y`), `expected.stderr` (the
//! warnings, as `line:col: level[code]: message`) and an empty `roundtrip`
//! marker (the expected output must survive a trip through the other
//! built-in profile and back).

use std::fs;
use std::path::{Path, PathBuf};

use mathfmt_core::{Config, Diagnostic, Engine, Level, LineIndex, RenameMap};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

struct Case {
    name: String,
    dir: PathBuf,
    input: String,
    expected: String,
    profile: Option<String>,
    macros: Vec<PathBuf>,
    rename: Option<(String, String)>,
    expected_stderr: Option<String>,
    roundtrip: bool,
}

/// The value after a flag in an `args` file, or a clear panic.
fn value<'a>(words: &[&'a str], i: usize, dir: &Path) -> &'a str {
    words
        .get(i + 1)
        .copied()
        .unwrap_or_else(|| panic!("{}: args: `{}` needs a value", dir.display(), words[i]))
}

fn load_case(dir: &Path) -> Case {
    let read = |name: &str| fs::read_to_string(dir.join(name)).ok();
    let mut profile = None;
    let mut macros = Vec::new();
    let mut rename = None;
    if let Some(args) = read("args") {
        let words: Vec<&str> = args
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let mut i = 0;
        let mut from = None;
        let mut to = None;
        while i < words.len() {
            match words[i] {
                "--profile" | "-p" => {
                    profile = Some(value(&words, i, dir).to_string());
                    i += 2;
                }
                "--macros" => {
                    macros.push(dir.join(value(&words, i, dir)));
                    i += 2;
                }
                "rename" => i += 1,
                "--from" => {
                    from = Some(value(&words, i, dir).to_string());
                    i += 2;
                }
                "--to" => {
                    to = Some(value(&words, i, dir).to_string());
                    i += 2;
                }
                other => panic!("{}: args: unknown `{other}`", dir.display()),
            }
        }
        if let (Some(f), Some(t)) = (from, to) {
            rename = Some((f, t));
        }
    }
    Case {
        name: dir.file_name().unwrap().to_string_lossy().into_owned(),
        dir: dir.to_path_buf(),
        input: read("input.tex").unwrap_or_else(|| panic!("{}: no input.tex", dir.display())),
        expected: read("expected.tex")
            .unwrap_or_else(|| panic!("{}: no expected.tex", dir.display())),
        profile,
        macros,
        rename,
        expected_stderr: read("expected.stderr"),
        roundtrip: dir.join("roundtrip").exists(),
    }
}

fn engine_for(case: &Case) -> Engine {
    let config_path = case.dir.join("mathfmt.toml");
    let config = if config_path.exists() {
        Config::load(&config_path).unwrap_or_else(|e| panic!("{}: {e}", case.name))
    } else {
        Config::default()
    };
    let mut engine = Engine::new(config).unwrap_or_else(|e| panic!("{}: {e}", case.name));
    for m in &case.macros {
        engine
            .add_macro_file(m)
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
    }
    engine
}

fn convert(
    engine: &Engine,
    case: &Case,
    src: &str,
    profile: Option<&str>,
) -> (String, Vec<Diagnostic>) {
    let profile = engine
        .profile(profile.or(case.profile.as_deref()))
        .unwrap_or_else(|e| panic!("{}: {e}", case.name));
    let mut rename = RenameMap::new();
    if let Some((from, to)) = &case.rename {
        let f = engine
            .parse_atom(from, &profile)
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let t = engine
            .parse_atom(to, &profile)
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        rename.insert(f, t);
    }
    let out = engine.convert(src, &profile, &rename);
    (out.output, out.diagnostics)
}

fn format_stderr(src: &str, diags: &[Diagnostic]) -> String {
    let index = LineIndex::new(src);
    let mut s = String::new();
    for d in diags {
        if d.level < Level::Warning {
            continue;
        }
        let d = d.clone().locate(None, &index);
        s.push_str(&d.to_string());
        s.push('\n');
    }
    s
}

#[test]
fn golden_fixtures() {
    let mut dirs: Vec<PathBuf> = fs::read_dir(fixtures_dir())
        .expect("tests/fixtures exists")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "no fixtures found");

    let mut failures = Vec::new();
    for dir in &dirs {
        let case = load_case(dir);
        let engine = engine_for(&case);
        let (output, diags) = convert(&engine, &case, &case.input, None);
        if output != case.expected {
            failures.push(format!(
                "{}: output differs\n--- expected\n{}--- actual\n{}",
                case.name, case.expected, output
            ));
            continue;
        }
        let stderr = format_stderr(&case.input, &diags);
        match &case.expected_stderr {
            Some(exp) if *exp != stderr => failures.push(format!(
                "{}: stderr differs\n--- expected\n{exp}--- actual\n{stderr}",
                case.name
            )),
            None if !stderr.is_empty() => {
                failures.push(format!("{}: unexpected warnings\n{stderr}", case.name))
            }
            _ => {}
        }
        // Idempotence: the expected output through the same profile is unchanged.
        let (again, _) = convert(&engine, &case, &case.expected, None);
        if again != case.expected {
            failures.push(format!(
                "{}: not idempotent\n--- expected\n{}--- second run\n{}",
                case.name, case.expected, again
            ));
        }
        if case.roundtrip {
            let here = case
                .profile
                .as_deref()
                .unwrap_or(engine.config.default_profile_name());
            let other = if here == "ascii" { "unicode" } else { "ascii" };
            let (there, _) = convert(&engine, &case, &case.expected, Some(other));
            let (back, _) = convert(&engine, &case, &there, Some(here));
            if back != case.expected {
                failures.push(format!(
                    "{}: round trip through {other} did not return\n--- {other}\n{there}--- back\n{back}",
                    case.name
                ));
            }
        }
    }
    if !failures.is_empty() {
        panic!(
            "{} of {} fixtures failed:\n\n{}",
            failures.len(),
            dirs.len(),
            failures.join("\n")
        );
    }
}
