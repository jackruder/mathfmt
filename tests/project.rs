//! Project mode over `tests/projects/<case>/` (design amendment of
//! 2026-11-09): a case is a small document tree with a `mathfmt.toml` that
//! names its `root`. Every file the root pulls in is converted under the
//! config's profile and compared with its twin under `expected/`, then
//! converted again to show the result is stable; what `mathfmt declarations`
//! prints is compared with `expected.declarations`. `tests/verify.rs`
//! compiles the same cases.

use std::fs;
use std::path::{Path, PathBuf};

use mathfmt_core::{Config, Engine, RenameMap};

fn projects_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/projects")
}

#[test]
fn projects_convert_as_expected() {
    let mut dirs: Vec<PathBuf> = fs::read_dir(projects_dir())
        .expect("tests/projects exists")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "no project cases");
    let mut failures = Vec::new();
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let config = Config::load(&dir.join("mathfmt.toml")).expect("the case's config");
        let engine = Engine::new(config).expect("the case's engine");
        let profile = engine.profile(None).expect("the case's profile");
        let project = engine
            .project
            .clone()
            .unwrap_or_else(|| panic!("{name}: the config names no root"));
        assert!(
            project.missing.is_empty(),
            "{name}: inputs with no file: {:?}",
            project.missing
        );
        let mut texts = Vec::new();
        for file in &project.files {
            let rel = file.strip_prefix(dir).expect("a file under the case");
            let src = fs::read_to_string(file).expect("a project file");
            let expected = fs::read_to_string(dir.join("expected").join(rel))
                .unwrap_or_else(|_| panic!("{name}: no expected/{}", rel.display()));
            let out = engine.convert(&src, &profile, &RenameMap::new());
            if out.output != expected {
                failures.push(format!(
                    "{name}/{}:\n--- expected\n{expected}--- got\n{}",
                    rel.display(),
                    out.output
                ));
            }
            let again = engine.convert(&expected, &profile, &RenameMap::new());
            if again.output != expected {
                failures.push(format!(
                    "{name}/{}: not stable; a second conversion gives\n{}",
                    rel.display(),
                    again.output
                ));
            }
            texts.push(src);
        }
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let declarations = engine.declarations(&refs, &profile, None).render();
        let expected = fs::read_to_string(dir.join("expected.declarations"))
            .unwrap_or_else(|_| panic!("{name}: no expected.declarations"));
        if declarations != expected {
            failures.push(format!(
                "{name}: declarations\n--- expected\n{expected}--- got\n{declarations}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} project checks failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
