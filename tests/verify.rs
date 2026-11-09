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

/// True (and says so) when the tools are missing and the test may skip.
fn skip_without_tex(what: &str) -> bool {
    if have("pdflatex") && have("pdftoppm") {
        return false;
    }
    if std::env::var_os("MATHFMT_VERIFY_REQUIRED").is_some() {
        panic!("MATHFMT_VERIFY_REQUIRED is set and pdflatex or pdftoppm is not on PATH");
    }
    eprintln!("verify: {what}: pdflatex or pdftoppm not on PATH; skipped");
    true
}

fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("mathfmt-verify-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for (path, text) in files {
        let p = dir.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    dir
}

fn verify_in(dir: &Path, args: &[&str]) -> (Option<i32>, String) {
    let out = project_command(dir, args)
        .arg("-q")
        .output()
        .expect("running mathfmt");
    (
        out.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

fn document(body: &str) -> String {
    format!("\\documentclass{{article}}\n\\begin{{document}}\n{body}\n\\end{{document}}\n")
}

/// Two documents side by side, a part of one, a snippet nothing reads and a
/// snippet that cannot compile: each root gets its verdict, the part is
/// verified with its document, and the failure takes nobody else's verdict.
#[test]
fn every_root_gets_a_verdict() {
    if skip_without_tex("several roots") {
        return;
    }
    let dir = tree(
        "roots",
        &[
            ("p1/main.tex", &document("$a \\le b$ \\input{sections/a}")),
            ("p1/sections/a.tex", "$c \\ne d$\n"),
            ("p2/main.tex", &document("$e \\ge f$")),
            ("other.tex", "$g \\to h$\n"),
            ("bad.tex", "$\\le \\undefinedmacro$\n"),
        ],
    );
    let (code, out) = verify_in(
        &dir,
        &[
            "--profile",
            "ascii",
            "--verify",
            "p1/main.tex",
            "p1/sections/a.tex",
            "p2/main.tex",
            "other.tex",
            "bad.tex",
        ],
    );
    for line in [
        "verified: p1/main.tex (1 page identical)",
        "verified: p2/main.tex (1 page identical)",
        "verified: other.tex (1 page identical)",
        "error: bad.tex: the original does not compile with pdflatex",
        "no document among the FILEs reads bad.tex",
    ] {
        assert!(out.contains(line), "missing `{line}` in:\n{out}");
    }
    assert!(
        !out.contains("sections/a.tex:"),
        "a part has no verdict of its own:\n{out}"
    );
    assert_eq!(
        code,
        Some(2),
        "an original that does not compile is an error:\n{out}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// `--keep DIR` may be relative and may be used again: the second run does
/// not read the first one's pages. A directory mathfmt did not make is
/// refused, not cleared.
#[test]
fn a_kept_build_is_relative_reusable_and_never_someone_elses() {
    if skip_without_tex("--keep") {
        return;
    }
    let dir = tree(
        "keep",
        &[(
            "doc.tex",
            &document("One $a \\le b$.\n\\newpage\nTwo $c \\ne d$."),
        )],
    );
    let args = [
        "--profile",
        "ascii",
        "--verify",
        "--keep",
        "kept",
        "doc.tex",
    ];
    let (code, out) = verify_in(&dir, &args);
    assert_eq!(code, Some(0), "{out}");
    assert!(
        out.contains("verified: doc.tex (2 pages identical)"),
        "{out}"
    );
    assert!(dir.join("kept/0-doc/a/out/doc.pdf").is_file());
    fs::write(dir.join("doc.tex"), document("One $a \\le b$.")).unwrap();
    let (code, out) = verify_in(&dir, &args);
    assert_eq!(code, Some(0), "{out}");
    assert!(
        out.contains("verified: doc.tex (1 page identical)"),
        "the first run's second page is not this run's:\n{out}"
    );
    fs::create_dir_all(dir.join("mine/0-doc")).unwrap();
    fs::write(dir.join("mine/0-doc/notes.txt"), "precious").unwrap();
    let (code, out) = verify_in(
        &dir,
        &[
            "--profile",
            "ascii",
            "--verify",
            "--keep",
            "mine",
            "doc.tex",
        ],
    );
    assert_eq!(code, Some(2), "{out}");
    assert!(out.contains("was not made by mathfmt"), "{out}");
    assert_eq!(
        fs::read_to_string(dir.join("mine/0-doc/notes.txt")).unwrap(),
        "precious"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A snippet's `--macros` package is found wherever it lives.
#[test]
fn a_snippet_finds_its_macro_package_in_another_directory() {
    if skip_without_tex("--macros") {
        return;
    }
    let dir = tree(
        "macros",
        &[
            ("shared/mydefs.sty", "\\newcommand{\\R}{\\mathbb{R}}\n"),
            ("work/s.tex", "$x \\in \\R$\n"),
        ],
    );
    let (code, out) = verify_in(
        &dir.join("work"),
        &["--verify", "--macros", "../shared/mydefs.sty", "s.tex"],
    );
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("verified: s.tex (1 page identical)"), "{out}");
    let _ = fs::remove_dir_all(&dir);
}

/// Two roots share a file whose conversion depends on each root's own
/// definitions: the first converts it, and the second's verdict must see
/// the converted copy, not the original through TEXINPUTS.
#[test]
fn a_file_two_roots_share_is_verified_as_converted_in_both() {
    if skip_without_tex("shared file") {
        return;
    }
    let dir = tree(
        "shared",
        &[
            ("a.tex", "\\documentclass{article}\n\\usepackage{unicode-math-input}\n\\newcommand{\\X}{\\alpha}\n\\begin{document}\nA: \\input{common}\n\\end{document}\n"),
            ("b.tex", "\\documentclass{article}\n\\usepackage{unicode-math-input}\n\\newcommand{\\X}{\\beta}\n\\begin{document}\nB: \\input{common}\n\\end{document}\n"),
            ("common.tex", "$\\X + 1$\n"),
        ],
    );
    let (code, out) = verify_in(&dir, &["--project", "a.tex", "b.tex", "--verify"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("verified: a.tex"), "{out}");
    assert!(
        out.contains("differs: b.tex"),
        "the second root sees the file as the first converted it:\n{out}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// In project mode a root without `\documentclass` is a snippet root and is
/// verified; a document another document reads (a subfile) is not a root;
/// `\documentclass` inside verbatim makes no document; an `\include` whose
/// target is not among the FILEs still gets its output directory; the
/// declarations go before a `\begin {document}` with a space.
#[test]
fn roots_are_chosen_as_the_spec_says() {
    if skip_without_tex("roots") {
        return;
    }
    let dir = tree(
        "roots2",
        &[
            ("snippet.tex", "Text $a \\le b$ \\input{part}\n"),
            ("part.tex", "$c \\ne d$\n"),
            ("sub/main.tex", "\\documentclass{article}\n\\usepackage{subfiles}\n\\usepackage{amssymb}\n\\begin{document}\n\\subfile{sec/part}\n\\include{ch/one}\n\\end{document}\n"),
            ("sub/sec/part.tex", "\\documentclass[../main.tex]{subfiles}\n\\begin{document}\n$x \\le y$\n\\end{document}\n"),
            ("sub/ch/one.tex", "$p \\ge q$\n"),
            ("verb.tex", "\\begin{verbatim}\n\\documentclass{article}\n\\end{verbatim}\n$\\alpha \\le 1$\n"),
            ("spaced.tex", "\\documentclass{article}\n\\begin {document}\n$\\alpha$\n\\end{document}\n"),
            ("nobegin.tex", "\\documentclass{article}\n% \\begin{document}\n"),
        ],
    );
    let (code, out) = verify_in(
        &dir,
        &["--profile", "ascii", "--project", "snippet.tex", "--verify"],
    );
    assert_eq!(code, Some(0), "{out}");
    assert!(
        out.contains("verified: snippet.tex (1 page identical)"),
        "{out}"
    );
    let (code, out) = verify_in(
        &dir,
        &[
            "--profile",
            "ascii",
            "--verify",
            "sub/main.tex",
            "sub/sec/part.tex",
            "verb.tex",
        ],
    );
    assert_eq!(code, Some(0), "{out}");
    assert!(
        out.contains("verified: sub/main.tex (2 pages identical)"),
        "an \\include starts a page:\n{out}"
    );
    assert!(
        out.contains("verified: verb.tex (1 page identical)"),
        "{out}"
    );
    assert!(
        !out.contains("sec/part.tex:"),
        "a subfile is verified with its document:\n{out}"
    );
    let (code, out) = verify_in(
        &dir,
        &["--verify", "--declare", "spaced.tex", "nobegin.tex"],
    );
    assert_eq!(code, Some(2), "{out}");
    assert!(
        out.contains("verified: spaced.tex (1 page identical)\n  with 2 declaration lines added"),
        "{out}"
    );
    assert!(
        out.contains("error: nobegin.tex:"),
        "a document with no live \\begin{{document}} is an error, not a pass:\n{out}"
    );
    let _ = fs::remove_dir_all(&dir);
}
