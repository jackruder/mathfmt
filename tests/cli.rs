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
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
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
