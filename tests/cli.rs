//! The binary's interface: filter mode, exit codes, --check, --write, --emit.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn mathfmt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mathfmt"))
}

fn run_stdin(args: &[&str], input: &str) -> (i32, String, String) {
    let mut child = mathfmt()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mathfmt");
    // A usage error exits before stdin is read; the pipe may already be
    // closed, which is not this test's concern.
    match child.stdin.take().unwrap().write_all(input.as_bytes()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(e) => panic!("writing stdin: {e}"),
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mathfmt-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn filter_mode_converts_stdin() {
    let (code, out, err) = run_stdin(&[], "$\\alpha \\in \\mathbb{R}$\n");
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "$α ∈ ℝ$\n");
    assert!(err.is_empty(), "{err}");
}

#[test]
fn profile_flag_and_strict_warnings() {
    let (code, out, _) = run_stdin(&["--profile", "ascii"], "$α ∈ ℝ$\n");
    assert_eq!(code, 0);
    assert_eq!(out, "$\\alpha \\in \\mathbb{R}$\n");
    let (code, out, err) = run_stdin(&["--profile", "ascii", "--strict"], "$∆$\n");
    assert_eq!(code, 1);
    assert_eq!(out, "$∆$\n");
    assert!(err.contains("no-latex-spelling"), "{err}");
    let (code, _, err) = run_stdin(&["--profile", "nope"], "$x$");
    assert_eq!(code, 2);
    assert!(err.contains("no profile named"), "{err}");
}

#[test]
fn check_write_and_diff_on_files() {
    let dir = temp_dir("files");
    let a = dir.join("a.tex");
    let b = dir.join("b.tex");
    fs::write(&a, "$\\alpha$\n").unwrap();
    fs::write(&b, "$β$\n").unwrap();

    let out = mathfmt().arg("--check").arg(&a).arg(&b).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("would change:") && stdout.contains("a.tex"),
        "{stdout}"
    );
    assert!(!stdout.contains("b.tex"), "{stdout}");
    assert_eq!(
        fs::read_to_string(&a).unwrap(),
        "$\\alpha$\n",
        "--check never writes"
    );

    let out = mathfmt().arg("--diff").arg(&a).output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("-$\\alpha$") && stdout.contains("+$α$"),
        "{stdout}"
    );

    let out = mathfmt().arg("--write").arg(&a).arg(&b).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(fs::read_to_string(&a).unwrap(), "$α$\n");
    assert_eq!(fs::read_to_string(&b).unwrap(), "$β$\n");
    assert!(
        !dir.join(".a.tex.mathfmt~").exists(),
        "no temp file left behind"
    );

    let out = mathfmt().arg("--check").arg(&a).arg(&b).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "clean after --write");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn nearest_config_is_found_per_file() {
    let dir = temp_dir("config");
    fs::write(dir.join("mathfmt.toml"), "profile = \"ascii\"\n").unwrap();
    let sub = dir.join("sections");
    fs::create_dir_all(&sub).unwrap();
    let f = sub.join("s.tex");
    fs::write(&f, "$α$\n").unwrap();
    let out = mathfmt().arg(&f).output().unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "$\\alpha$\n");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn emit_symbols_and_rename() {
    let (code, out, _) = run_stdin(&["--emit", "symbols"], "$\\mathbb{R} ℝ \\in$\n");
    assert_eq!(code, 0);
    let lines: Vec<serde_json::Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    let reals = lines.iter().find(|v| v["unicode"] == "ℝ").expect("ℝ row");
    assert_eq!(reals["count"], 2);
    assert_eq!(reals["spellings"]["\\mathbb{R}"], 1);

    let (code, out, _) = run_stdin(
        &["rename", "--from", "\\mathfrak{F}", "--to", "ℱ"],
        "$\\mathfrak{F} 𝔉$\n",
    );
    assert_eq!(code, 0);
    assert_eq!(out, "$ℱ ℱ$\n");

    let (code, _, err) = run_stdin(&["rename", "--from", "xy", "--to", "ℱ"], "$x$");
    assert_eq!(code, 2);
    assert!(err.contains("exactly one symbol"), "{err}");
}

#[test]
fn table_dumps_tsv() {
    let out = mathfmt().arg("table").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.starts_with("codepoint\tatom\tunicode\tlatex\tprofile:unicode\tname\taliases\n"));
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("2208\t∈\t∈\t\\in\t∈\t")),
        "the ∈ row"
    );
}

#[test]
fn macros_flag_takes_one_file_and_leaves_the_positionals() {
    let dir = temp_dir("macros");
    let defs = dir.join("defs.sty");
    let f = dir.join("f.tex");
    fs::write(&defs, "\\newcommand{\\R}{\\mathbb{R}}\n").unwrap();
    fs::write(&f, "$x \\in \\R$\n").unwrap();
    let out = mathfmt()
        .arg("--macros")
        .arg(&defs)
        .arg(&f)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "$x ∈ ℝ$\n");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn strict_counts_warnings_even_under_quiet() {
    let (code, out, err) = run_stdin(&["--profile", "ascii", "--strict", "-q"], "$∆$\n");
    assert_eq!(code, 1);
    assert_eq!(out, "$∆$\n");
    assert!(err.is_empty(), "{err}");
}

#[test]
fn write_needs_files_and_excludes_diff() {
    let (code, _, err) = run_stdin(&["--write"], "$\\alpha$\n");
    assert_eq!(code, 2, "{err}");
    assert!(!err.contains("panicked"), "{err}");
    let dir = temp_dir("write-diff");
    let f = dir.join("f.tex");
    fs::write(&f, "$\\alpha$\n").unwrap();
    let out = mathfmt()
        .arg("--write")
        .arg("--diff")
        .arg(&f)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(&f).unwrap(),
        "$\\alpha$\n",
        "nothing written"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn write_follows_symlinks() {
    let dir = temp_dir("symlink");
    let real = dir.join("real.tex");
    let link = dir.join("link.tex");
    fs::write(&real, "$\\alpha$\n").unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let out = mathfmt().arg("--write").arg(&link).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(&real).unwrap(),
        "$α$\n",
        "the target changed"
    );
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link is still a link"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn emit_keeps_argument_order_across_configs() {
    let dir = temp_dir("emit-order");
    let a = dir.join("a");
    let b = dir.join("b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    fs::write(b.join("mathfmt.toml"), "profile = \"ascii\"\n").unwrap();
    fs::write(b.join("2.tex"), "$α$\n").unwrap();
    fs::write(a.join("1.tex"), "$β$\n").unwrap();
    fs::write(a.join("3.tex"), "$γ$\n").unwrap();
    let out = mathfmt()
        .arg("--emit")
        .arg("symbols")
        .arg(b.join("2.tex"))
        .arg(a.join("1.tex"))
        .arg(a.join("3.tex"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let files: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["file"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert!(
        files[0].ends_with("2.tex") && files[1].ends_with("1.tex") && files[2].ends_with("3.tex"),
        "{files:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

fn write_tree(dir: &std::path::Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let p = dir.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
}

fn run_in(dir: &std::path::Path, args: &[&str]) -> (i32, String, String) {
    let out = mathfmt()
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn project_mode_takes_files_and_definitions_from_the_root() {
    let dir = temp_dir("project");
    write_tree(
        &dir,
        &[
            (
                "main.tex",
                "\\documentclass{article}\n\\usepackage{local}\n\\begin{document}\n$\\alpha$\n\\input{sections/a}\n\\end{document}\n",
            ),
            ("local.sty", "\\newcommand{\\R}{\\mathbb{R}}\n"),
            ("sections/a.tex", "$x \\in \\R$\n"),
            ("sections/unused.tex", "$\\beta$\n"),
        ],
    );
    // The files are the root and what it inputs, in reading order.
    let (code, out, err) = run_in(&dir, &["--project", "main.tex", "--check"]);
    assert_eq!(code, 1, "{err}");
    assert_eq!(
        out,
        "would change: main.tex\nwould change: sections/a.tex\n"
    );
    // Alone, the section does not know the package's \R; under its root it does.
    let (_, out, _) = run_in(&dir, &["sections/a.tex"]);
    assert_eq!(out, "$x ∈ \\R$\n");
    let (code, out, err) = run_in(&dir, &["--root", "main.tex", "sections/a.tex"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "$x ∈ ℝ$\n");
    // `root` in the config does both: context for a lone file, and the
    // files of --project with no FILE, from any directory below it.
    fs::write(dir.join("mathfmt.toml"), "root = \"main.tex\"\n").unwrap();
    let (_, out, _) = run_in(&dir.join("sections"), &["a.tex"]);
    assert_eq!(out, "$x ∈ ℝ$\n");
    let (code, _, err) = run_in(&dir, &["--project", "--write"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        fs::read_to_string(dir.join("sections/a.tex")).unwrap(),
        "$x ∈ ℝ$\n"
    );
    assert!(fs::read_to_string(dir.join("main.tex"))
        .unwrap()
        .contains("$α$"));
    assert_eq!(
        fs::read_to_string(dir.join("local.sty")).unwrap(),
        "\\newcommand{\\R}{\\mathbb{R}}\n",
        "a package is read, never written"
    );
    assert_eq!(
        fs::read_to_string(dir.join("sections/unused.tex")).unwrap(),
        "$\\beta$\n",
        "a file the root does not input is not the project's"
    );
    let (code, out, _) = run_in(&dir, &["--project", "--check"]);
    assert_eq!((code, out.as_str()), (0, ""));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn project_mode_needs_a_root_that_exists() {
    let dir = temp_dir("project-root");
    write_tree(&dir, &[("a.tex", "$\\alpha$\n")]);
    let (code, out, err) = run_in(&dir, &["--project"]);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.contains("--project needs a root document"), "{err}");
    let (code, _, err) = run_in(&dir, &["--project", "absent.tex"]);
    assert_eq!(code, 2);
    assert!(err.contains("absent.tex"), "{err}");
    let (code, _, err) = run_in(&dir, &["--root", "absent.tex", "a.tex"]);
    assert_eq!(code, 2);
    assert!(err.contains("absent.tex"), "{err}");
    fs::write(dir.join("mathfmt.toml"), "root = \"absent.tex\"\n").unwrap();
    let (code, _, err) = run_in(&dir, &["a.tex"]);
    assert_eq!(code, 2, "a config whose root is missing is an error");
    assert!(err.contains("absent.tex"), "{err}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn declarations_and_project_say_what_the_root_has_and_needs() {
    let dir = temp_dir("declarations");
    write_tree(
        &dir,
        &[(
            "main.tex",
            "\\documentclass{article}\n\\usepackage{amsmath}\n\\begin{document}\n$\\mathbb{R} \\ni x \\leqslant \\mathbbm{1}$\n\\end{document}\n",
        )],
    );
    let (code, out, err) = run_in(&dir, &["project", "main.tex"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        "root\tmain.tex\nengine\tpdflatex\nunicode-input\tunicode-math-input\nclass\tarticle\nfile\tmain.tex\npackage\tamsmath\nmacros\t0\n"
    );
    let (code, out, err) = run_in(
        &dir,
        &["declarations", "--project", "main.tex", "--with", "kernel"],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        "% mathfmt declarations: main.tex, profile unicode, kernel\n\
         % packages the notation's commands need and the preamble does not load\n\
         \\usepackage{amssymb}  % \\leqslant \\mathbb\n\
         \\usepackage{bbm}  % \\mathbbm\n\
         % Unicode math input: each character means what the profile spells it as\n\
         \\DeclareUnicodeCharacter{211D}{\\ensuremath{\\mathbb{R}}}  % ℝ\n\
         \\DeclareUnicodeCharacter{220B}{\\ensuremath{\\ni}}  % ∋\n\
         \\DeclareUnicodeCharacter{2A7D}{\\ensuremath{\\leqslant}}  % ⩽\n\
         \\DeclareUnicodeCharacter{1D7D9}{\\ensuremath{\\mathbbm{1}}}  % 𝟙\n"
    );
    // Without a root there is no preamble to subtract from, and the
    // default mechanism under pdflatex is unicode-math-input, which knows
    // the symbols itself.
    let (code, out, _) = run_stdin(&["declarations"], "$α ∈ ℝ$ and $∆$\n");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "% mathfmt declarations: <stdin>, profile unicode, unicode-math-input\n\
         % packages the notation's commands need\n\
         \\usepackage{amsfonts}  % \\mathbb\n\
         \\usepackage{unicode-math-input}\n\
         % Unicode math input: each character means what the profile spells it as (unicode-math-input's own table lacks some and reads others differently)\n\
         \\umiDeclareMathChar{α}{\\alpha}\n\
         \\umiDeclareMathChar{ℝ}{\\mathbb{R}}\n\
         \\umiDeclareMathChar{∈}{\\in}\n\
         % ∆ (U+2206) is not declared: the tables have no LaTeX spelling for it; declare it by hand\n"
    );
    let (code, out, _) = run_stdin(&["declarations"], "no math here\n");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "% mathfmt declarations: <stdin>, profile unicode, unicode-math-input\n% nothing to add\n"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn emit_lists_notations_and_whether_each_is_rewritten() {
    let dir = temp_dir("emit-notations");
    write_tree(
        &dir,
        &[
            (
                "mathfmt.toml",
                "[notations.KL]\nmacro = \"\\\\KL{#1}{#2}\"\nexpanded = \"D_{\\\\mathrm{KL}}(#1 \\\\| #2)\"\n[profiles.unicode.notations]\nKL = \"expanded\"\n",
            ),
            ("a.tex", "$\\KL{p}{q} + e^\\KL{a}{b} + D_{\\mathrm{KL}}(r \\| s)$\n"),
        ],
    );
    let (code, out, err) = run_in(&dir, &["--emit", "symbols", "a.tex"]);
    assert_eq!(code, 0, "{err}");
    let notation: Vec<serde_json::Value> = out
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|v| v.get("notation").is_some())
        .collect();
    assert_eq!(notation.len(), 1, "{out}");
    assert_eq!(notation[0]["notation"], "KL");
    assert_eq!(notation[0]["count"], 3);
    assert_eq!(notation[0]["forms"]["macro"], 2);
    assert_eq!(notation[0]["forms"]["expanded"], 1);
    let (_, out, _) = run_in(&dir, &["--emit", "atoms", "a.tex"]);
    let matches: Vec<serde_json::Value> = out
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|v| v["kind"] == "notation")
        .collect();
    let seen: Vec<(String, bool)> = matches
        .iter()
        .map(|m| {
            (
                m["source"].as_str().unwrap().to_string(),
                m["rewritten"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("\\KL{p}{q}".to_string(), true),
            ("\\KL{a}{b}".to_string(), false),
            ("D_{\\mathrm{KL}}(r \\| s)".to_string(), true),
        ],
        "a notation that is a script operand is seen and kept"
    );
    assert_eq!(matches[0]["holes"], serde_json::json!(["p", "q"]));
    // A rename leaves the notation's shape alone and reaches into its holes.
    let (code, out, err) = run_in(&dir, &["rename", "--from", "p", "--to", "p", "a.tex"]);
    assert_eq!(
        code, 2,
        "a plain letter is not a symbol to rename: {out}{err}"
    );
    fs::write(dir.join("b.tex"), "$\\KL{\\alpha}{q}$\n").unwrap();
    let (code, out, err) = run_in(
        &dir,
        &["rename", "--from", "\\alpha", "--to", "\\beta", "b.tex"],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "$D_{\\mathrm{KL}}(β ‖ q)$\n");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_document_on_its_own_and_the_engine_flag_pick_the_unicode_math_commands() {
    let dir = temp_dir("own-root");
    write_tree(
        &dir,
        &[
            (
                "main.tex",
                "% !TEX program = lualatex\n\\documentclass{article}\n\\usepackage{unicode-math}\n\\begin{document}\n$𝐱 ∈ ℝ$\n\\end{document}\n",
            ),
            ("s.tex", "$𝐱 ∈ ℝ$\n"),
        ],
    );
    let (code, out, err) = run_in(&dir, &["-p", "ascii", "main.tex"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("$\\symbf{x} \\in \\symbb{R}$"),
        "a document alone is its own root:\n{out}"
    );
    let (_, out, _) = run_in(&dir, &["-p", "ascii", "s.tex"]);
    assert_eq!(
        out, "$\\mathbf{x} \\in \\mathbb{R}$\n",
        "a snippet assumes pdflatex"
    );
    let (_, out, _) = run_in(&dir, &["-p", "ascii", "--engine", "lualatex", "s.tex"]);
    assert_eq!(
        out, "$\\symbf{x} \\in \\symbb{R}$\n",
        "--engine says what the commands assume"
    );
    let (_, out, _) = run_in(&dir, &["declarations", "--engine", "lualatex", "s.tex"]);
    assert!(out.contains("unicode-math\n"), "{out}");
    assert!(!out.contains("unicode-math-input"), "{out}");
    fs::write(dir.join("mathfmt.toml"), "engine = \"lualatex\"\n").unwrap();
    let (_, out, _) = run_in(&dir, &["-p", "ascii", "s.tex"]);
    assert_eq!(
        out, "$\\symbf{x} \\in \\symbb{R}$\n",
        "so does `engine` in the config"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn options_before_a_subcommand_are_refused() {
    let (code, out, err) = run_stdin(&["-p", "ascii", "declarations"], "$α$\n");
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(
        err.contains("before the subcommand `declarations`"),
        "{err}"
    );
    let (code, out, _) = run_stdin(&["declarations", "-p", "ascii"], "$α$\n");
    assert_eq!(code, 0);
    assert!(
        out.starts_with("% mathfmt declarations: <stdin>, profile ascii"),
        "{out}"
    );
}
