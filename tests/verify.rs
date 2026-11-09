//! Compile-and-compare over the golden fixtures (design §14, amendment of
//! 2026-11-08): each case's `input.tex` and its conversion must render the
//! same pages under `mathfmt --verify`, unless the case's `expected.verify`
//! says `differs` (a rename, or a repair that changes the typeset result on
//! purpose) or `skip`, each followed by the reason.
//!
//! The project cases under `tests/projects/` are compiled the same way, as
//! documents: `mathfmt --project --verify --declare`, with the engine the
//! case's preamble asks for.
//!
//! Needs pdflatex and pdftoppm on PATH. Without them the test prints that it
//! skipped and passes, unless `MATHFMT_VERIFY_REQUIRED` is set, as the
//! flake's `verify` check sets it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

#[derive(Debug, PartialEq, Eq)]
enum Expect {
    Identical,
    Differs,
    Skip,
}

fn expectation(dir: &Path) -> (Expect, String) {
    match fs::read_to_string(dir.join("expected.verify")) {
        Ok(text) => {
            let text = text.trim();
            let (word, reason) = text.split_once(':').unwrap_or((text, ""));
            let e = match word.trim() {
                "identical" => Expect::Identical,
                "differs" => Expect::Differs,
                "skip" => Expect::Skip,
                other => panic!(
                    "{}: expected.verify says `{other}`; it must be identical, differs or skip",
                    dir.display()
                ),
            };
            if e != Expect::Identical && reason.trim().is_empty() {
                panic!(
                    "{}: expected.verify must give a reason after a colon",
                    dir.display()
                );
            }
            (e, reason.trim().to_string())
        }
        Err(_) => (Expect::Identical, String::new()),
    }
}

/// The command line for a case: `rename` and its flags first, then
/// `--verify` and the run flags from `args`, then the input.
fn command_for(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mathfmt"));
    cmd.current_dir(dir);
    let mut run_flags: Vec<String> = Vec::new();
    if let Ok(args) = fs::read_to_string(dir.join("args")) {
        let words: Vec<&str> = args
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let mut i = 0;
        while i < words.len() {
            match words[i] {
                "rename" => {
                    cmd.arg("rename");
                    i += 1;
                }
                "--from" | "--to" => {
                    cmd.arg(words[i]).arg(words[i + 1]);
                    i += 2;
                }
                flag => {
                    run_flags.push(flag.to_string());
                    run_flags.push(words[i + 1].to_string());
                    i += 2;
                }
            }
        }
    }
    cmd.arg("--verify").arg("-q");
    cmd.args(run_flags);
    cmd.arg("input.tex");
    cmd
}

#[test]
fn fixtures_render_the_same_after_conversion() {
    let required = std::env::var_os("MATHFMT_VERIFY_REQUIRED").is_some();
    for tool in ["pdflatex", "pdftoppm"] {
        if !have(tool) {
            if required {
                panic!("MATHFMT_VERIFY_REQUIRED is set and `{tool}` is not on PATH");
            }
            eprintln!("verify: `{tool}` not on PATH; skipped");
            return;
        }
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(fixtures_dir())
        .expect("tests/fixtures exists")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    // Every case compiles twice; run them side by side.
    let results: Vec<(String, Result<(), String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = dirs
            .iter()
            .map(|dir| {
                scope.spawn(move || {
                    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
                    (name, check_case(dir, command_for(dir)))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut failures = Vec::new();
    let mut verified = 0;
    for (name, r) in results {
        match r {
            Ok(()) => verified += 1,
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} fixtures failed to verify:\n\n{}",
        failures.len(),
        dirs.len(),
        failures.join("\n\n")
    );
    eprintln!("verify: {verified} fixtures compared");
}

fn projects_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/projects")
}

fn project_command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mathfmt"));
    cmd.current_dir(dir).args(args);
    cmd
}

/// The engine `mathfmt project` reads from the case's root.
fn project_engine(dir: &Path) -> String {
    let out = project_command(dir, &["project"])
        .output()
        .expect("running mathfmt project");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("engine\t").map(str::to_string))
        .unwrap_or_else(|| panic!("{}: mathfmt project names no engine", dir.display()))
}

#[test]
fn projects_render_the_same_after_conversion() {
    let required = std::env::var_os("MATHFMT_VERIFY_REQUIRED").is_some();
    if !have("pdftoppm") {
        if required {
            panic!("MATHFMT_VERIFY_REQUIRED is set and `pdftoppm` is not on PATH");
        }
        eprintln!("verify: `pdftoppm` not on PATH; skipped");
        return;
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(projects_dir())
        .expect("tests/projects exists")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let mut failures = Vec::new();
    let mut verified = 0;
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let engine = project_engine(dir);
        if !have(&engine) {
            if required {
                panic!("MATHFMT_VERIFY_REQUIRED is set and `{engine}` is not on PATH");
            }
            eprintln!("verify: {name}: `{engine}` not on PATH; skipped");
            continue;
        }
        let cmd = project_command(dir, &["--project", "--verify", "--declare", "-q"]);
        match check_case(dir, cmd) {
            Ok(()) => verified += 1,
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} projects failed to verify:\n\n{}",
        failures.len(),
        dirs.len(),
        failures.join("\n\n")
    );
    eprintln!("verify: {verified} projects compared");
}

/// Without the declarations in its preamble the Unicode side of a pdflatex
/// document does not compile, and the verdict says what to run.
#[test]
fn a_preamble_that_reads_no_unicode_is_broken_and_says_so() {
    if !have("pdflatex") || !have("pdftoppm") {
        if std::env::var_os("MATHFMT_VERIFY_REQUIRED").is_some() {
            panic!("MATHFMT_VERIFY_REQUIRED is set and pdflatex or pdftoppm is not on PATH");
        }
        eprintln!("verify: pdflatex or pdftoppm not on PATH; skipped");
        return;
    }
    let out = project_command(
        &projects_dir().join("paper"),
        &["--project", "--verify", "-q"],
    )
    .output()
    .expect("running mathfmt");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.starts_with("broken: main.tex"), "{stdout}");
    assert!(stdout.contains("mathfmt declarations"), "{stdout}");
}

fn check_case(dir: &Path, mut command: Command) -> Result<(), String> {
    let (expect, reason) = expectation(dir);
    if expect == Expect::Skip {
        eprintln!("verify: {} skipped: {reason}", dir.display());
        return Ok(());
    }
    let out = command
        .output()
        .map_err(|e| format!("running mathfmt: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let first = stdout.lines().next().unwrap_or("");
    let verdict = first.split(':').next().unwrap_or("");
    let got = match verdict {
        "verified" | "unchanged" => Expect::Identical,
        "differs" => Expect::Differs,
        _ => {
            return Err(format!(
                "no verdict (exit {:?})\nstdout:\n{stdout}stderr:\n{stderr}",
                out.status.code()
            ))
        }
    };
    if got != expect {
        return Err(format!(
            "expected {expect:?}{}, got:\n{stdout}{stderr}",
            if reason.is_empty() {
                String::new()
            } else {
                format!(" ({reason})")
            }
        ));
    }
    let want_exit = if expect == Expect::Differs {
        Some(1)
    } else {
        Some(0)
    };
    if out.status.code() != want_exit {
        return Err(format!(
            "exit {:?}, wanted {want_exit:?}\n{stdout}{stderr}",
            out.status.code()
        ));
    }
    Ok(())
}
