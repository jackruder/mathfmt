//! `mathfmt.toml` and the profiles it defines (design §9, §10).

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::atom::Style;

pub const CONFIG_FILE_NAME: &str = "mathfmt.toml";

#[derive(Debug)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// Whether a class of atom prints as a Unicode character or a LaTeX command.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Form {
    Unicode,
    Latex,
}

/// The family of commands a profile spells the math alphabets with, before
/// its own `style_commands`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StylePreset {
    /// `\mathbf`, `\boldsymbol`, `\mathbb`, …: the kernel, amsmath, amssymb
    /// and bm, as pdflatex typesets them.
    Latex,
    /// `\symbf`, `\symbfit`, `\symbb`, …: the unicode-math package, where
    /// `\mathbf` is the text font's bold and `\boldsymbol` is not bold.
    UnicodeMath,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    pub symbols: Option<Form>,
    pub styled: Option<Form>,
    pub greek: Option<Form>,
    pub inline_macros: Option<bool>,
    pub script_reads_as: Option<String>,
    /// Default: `unicode-math` when the project's preamble loads that
    /// package, `latex` otherwise.
    pub style_preset: Option<StylePreset>,
    #[serde(default)]
    pub style_commands: BTreeMap<String, String>,
    #[serde(default)]
    pub spellings: BTreeMap<String, String>,
    /// notation name → the form this profile writes it in.
    #[serde(default)]
    pub notations: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolDecl {
    pub atom: String,
    pub means: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_profile_name")]
    pub profile: String,
    #[serde(default)]
    pub macros: Vec<String>,
    /// The project's root document, relative to this file: its preamble's
    /// packages and definitions are in force for every file converted.
    pub root: Option<String>,
    /// The program `--verify` compiles with (default: what the root's
    /// preamble says, else `pdflatex`).
    pub engine: Option<String>,
    /// How the document reads Unicode math, for `mathfmt declarations`
    /// (default: `unicode-math` when the preamble loads it or the engine is
    /// not pdflatex, else `unicode-math-input`).
    pub unicode_input: Option<crate::declare::Mechanism>,
    #[serde(default)]
    pub math_environments: Vec<String>,
    #[serde(default)]
    pub verbatim_environments: Vec<String>,
    #[serde(default)]
    pub text_commands: Vec<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileConfig>,
    #[serde(default)]
    pub symbols: BTreeMap<String, SymbolDecl>,
    /// notation name → form name → pattern with `#n` holes. Every form is
    /// recognized on input; a profile's `notations` picks the one it writes.
    #[serde(default)]
    pub notations: BTreeMap<String, BTreeMap<String, String>>,
    /// Where the file was found; `macros` paths are relative to it.
    #[serde(skip)]
    pub dir: Option<PathBuf>,
}

fn default_version() -> u32 {
    1
}

fn default_profile_name() -> String {
    "unicode".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: 1,
            profile: default_profile_name(),
            macros: Vec::new(),
            root: None,
            engine: None,
            unicode_input: None,
            math_environments: Vec::new(),
            verbatim_environments: Vec::new(),
            text_commands: Vec::new(),
            profiles: BTreeMap::new(),
            symbols: BTreeMap::new(),
            notations: BTreeMap::new(),
            dir: None,
        }
    }
}

/// A profile with every field decided.
#[derive(Clone, Debug)]
pub struct Profile {
    pub name: String,
    pub symbols: Form,
    pub styled: Form,
    pub greek: Form,
    pub inline_macros: bool,
    pub script_reads_as: Style,
    /// The family its `style_commands` started from; under `unicode-math`
    /// a symbol whose curated spelling that package lacks is spelled by
    /// unicode-math's own name.
    pub preset: StylePreset,
    /// style name (or `bf_greek`, `bb_digit`, `bb_lower`) → command,
    /// possibly with `#1`.
    pub style_commands: HashMap<String, String>,
    /// atom key or character → spelling.
    pub spellings: HashMap<String, String>,
    /// notation name → the pattern of the form this profile writes. A
    /// notation absent here is recognized but left in whatever form it has.
    pub notations: HashMap<String, String>,
}

impl Profile {
    /// This profile with every class of atom printed as a command: what each
    /// of its Unicode characters is spelled as in LaTeX.
    pub fn latex_forms(&self) -> Profile {
        Profile {
            symbols: Form::Latex,
            styled: Form::Latex,
            greek: Form::Latex,
            ..self.clone()
        }
    }
}

/// The unicode-math commands, as the package's default options read them:
/// the shape of a bold or sans letter is the document's (`bold-style`,
/// `sans-style`), so a literal 𝛔 and `\symbf{\sigma}` are the same house
/// bold sigma and `bf` is `\symbf`, not `\symbfup`. `bfit`, `sfit` and
/// `bfsfit` name their shape, which a literal character cannot under those
/// options.
const UNICODE_MATH_STYLE_COMMANDS: &[(&str, &str)] = &[
    ("up", "\\symup"),
    ("it", "\\symit"),
    ("bf", "\\symbf"),
    ("bf_greek", "\\symbf"),
    ("bfit", "\\symbfit"),
    ("bb", "\\symbb"),
    ("bb_digit", "\\symbb"),
    ("bb_lower", "\\symbb"),
    ("cal", "\\symcal"),
    ("scr", "\\symscr"),
    ("bfcal", "\\symbfcal"),
    ("bfscr", "\\symbfscr"),
    ("frak", "\\symfrak"),
    ("bffrak", "\\symbffrak"),
    ("sf", "\\symsf"),
    ("sfit", "\\symsfit"),
    ("bfsf", "\\symbfsf"),
    ("bfsfit", "\\symbfsfit"),
    ("tt", "\\symtt"),
];

const DEFAULT_STYLE_COMMANDS: &[(&str, &str)] = &[
    ("up", "\\mathrm"),
    ("it", "\\mathit"),
    ("bf", "\\mathbf"),
    ("bf_greek", "\\boldsymbol"),
    ("bfit", "\\boldsymbol"),
    ("bb", "\\mathbb"),
    // amsfonts has neither blackboard digits nor lowercase (`\mathbb{a}`
    // is some other glyph); bbm has both.
    ("bb_digit", "\\mathbbm"),
    ("bb_lower", "\\mathbbm"),
    ("cal", "\\mathcal"),
    ("scr", "\\mathscr"),
    ("bfcal", "\\boldsymbol{\\mathcal{#1}}"),
    ("bfscr", "\\boldsymbol{\\mathscr{#1}}"),
    ("frak", "\\mathfrak"),
    ("bffrak", "\\boldsymbol{\\mathfrak{#1}}"),
    ("sf", "\\mathsf"),
    ("bfsf", "\\boldsymbol{\\mathsf{#1}}"),
    ("tt", "\\mathtt"),
];

const STYLE_COMMAND_KEYS: &[&str] = &["bf_greek", "bb_digit", "bb_lower"];

impl Config {
    /// Parse a config file's text. `dir` is where relative `macros` resolve.
    pub fn parse(text: &str, dir: Option<&Path>) -> Result<Config, ConfigError> {
        let mut cfg: Config = toml::from_str(text).map_err(|e| ConfigError(format!("{e}")))?;
        cfg.dir = dir.map(Path::to_path_buf);
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ConfigError(format!("{}: {e}", path.display())))?;
        Config::parse(&text, path.parent())
            .map_err(|e| ConfigError(format!("{}: {e}", path.display())))
    }

    /// The nearest `mathfmt.toml` at or above `dir`.
    pub fn find(dir: &Path) -> Option<PathBuf> {
        let mut cur = Some(dir);
        while let Some(d) = cur {
            let candidate = d.join(CONFIG_FILE_NAME);
            if candidate.is_file() {
                return Some(candidate);
            }
            cur = d.parent();
        }
        None
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.version != 1 {
            return Err(ConfigError(format!(
                "version {} is not supported; this mathfmt reads version 1",
                self.version
            )));
        }
        for (key, value) in [("root", &self.root), ("engine", &self.engine)] {
            if value.as_deref().is_some_and(|v| v.trim().is_empty()) {
                return Err(ConfigError(format!("`{key}` is empty")));
            }
        }
        for (name, p) in &self.profiles {
            for key in p.style_commands.keys() {
                let known = Style::ALL.iter().any(|s| s.as_str() == key)
                    || STYLE_COMMAND_KEYS.contains(&key.as_str());
                if !known {
                    return Err(ConfigError(format!(
                        "profiles.{name}.style_commands: `{key}` is not a style"
                    )));
                }
            }
            if let Some(s) = &p.script_reads_as {
                if s != "cal" && s != "scr" {
                    return Err(ConfigError(format!(
                        "profiles.{name}.script_reads_as must be \"cal\" or \"scr\", not \"{s}\""
                    )));
                }
            }
            for (nname, fname) in &p.notations {
                let Some(forms) = self.notations.get(nname) else {
                    return Err(ConfigError(format!(
                        "profiles.{name}.notations: no notation named `{nname}`"
                    )));
                };
                if !forms.contains_key(fname) {
                    return Err(ConfigError(format!(
                        "profiles.{name}.notations.{nname}: no form named `{fname}`; known: {}",
                        forms.keys().cloned().collect::<Vec<_>>().join(", ")
                    )));
                }
            }
        }
        for (nname, forms) in &self.notations {
            if forms.is_empty() {
                return Err(ConfigError(format!("notations.{nname} has no forms")));
            }
            let mut holes: Option<usize> = None;
            for (fname, pattern) in forms {
                let n = crate::notation::check_pattern(pattern)
                    .map_err(|e| ConfigError(format!("notations.{nname}.{fname}: {e}")))?;
                match holes {
                    None => holes = Some(n),
                    Some(h) if h != n => {
                        return Err(ConfigError(format!(
                        "notations.{nname}: form `{fname}` has {n} holes where the others have {h}"
                    )))
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// The notations as the matcher wants them: name, then (form, pattern).
    pub fn notation_decls(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.notations
            .iter()
            .map(|(n, forms)| {
                (
                    n.clone(),
                    forms.iter().map(|(f, p)| (f.clone(), p.clone())).collect(),
                )
            })
            .collect()
    }

    /// The root document's path, when the config names one.
    pub fn root_path(&self) -> Option<PathBuf> {
        self.root.as_ref().map(|r| match &self.dir {
            Some(d) => d.join(r),
            None => PathBuf::from(r),
        })
    }

    /// Resolve a profile by name, with the built-in `unicode` and `ascii` as
    /// bases. A profile the file defines under another name starts from the
    /// `unicode` defaults.
    pub fn profile(&self, name: &str) -> Result<Profile, ConfigError> {
        self.profile_with(name, StylePreset::Latex)
    }

    /// As [`Config::profile`], with the style preset a profile gets when it
    /// does not choose one: the caller knows what the document loads.
    pub fn profile_with(&self, name: &str, preset: StylePreset) -> Result<Profile, ConfigError> {
        let user = self.profiles.get(name);
        let builtin = match name {
            "unicode" => Some(Form::Unicode),
            "ascii" => Some(Form::Latex),
            _ => None,
        };
        if user.is_none() && builtin.is_none() {
            let mut known: Vec<&str> = vec!["unicode", "ascii"];
            known.extend(self.profiles.keys().map(String::as_str));
            return Err(ConfigError(format!(
                "no profile named `{name}`; known: {}",
                known.join(", ")
            )));
        }
        let base = builtin.unwrap_or(Form::Unicode);
        let user = user.cloned().unwrap_or_default();
        let preset = user.style_preset.unwrap_or(preset);
        let mut style_commands: HashMap<String, String> = match preset {
            StylePreset::Latex => DEFAULT_STYLE_COMMANDS,
            StylePreset::UnicodeMath => UNICODE_MATH_STYLE_COMMANDS,
        }
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        style_commands.extend(user.style_commands.clone());
        Ok(Profile {
            name: name.to_string(),
            symbols: user.symbols.unwrap_or(base),
            styled: user.styled.unwrap_or(base),
            greek: user.greek.unwrap_or(base),
            inline_macros: user.inline_macros.unwrap_or(true),
            script_reads_as: match user.script_reads_as.as_deref() {
                Some("scr") => Style::Scr,
                _ => Style::Cal,
            },
            preset,
            style_commands,
            spellings: user.spellings.into_iter().collect(),
            notations: user
                .notations
                .iter()
                .filter_map(|(n, f)| {
                    self.notations
                        .get(n)
                        .and_then(|forms| forms.get(f))
                        .map(|p| (n.clone(), p.clone()))
                })
                .collect(),
        })
    }

    pub fn default_profile_name(&self) -> &str {
        &self.profile
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_builtins() {
        let c = Config::default();
        let u = c.profile("unicode").unwrap();
        assert_eq!(
            (u.symbols, u.styled, u.greek, u.inline_macros),
            (Form::Unicode, Form::Unicode, Form::Unicode, true)
        );
        let a = c.profile("ascii").unwrap();
        assert_eq!(
            (a.symbols, a.styled, a.greek),
            (Form::Latex, Form::Latex, Form::Latex)
        );
        assert_eq!(a.style_commands["bf_greek"], "\\boldsymbol");
        assert!(c.profile("nope").is_err());
    }

    #[test]
    fn user_profile_overrides_fields_and_keeps_the_rest() {
        let c = Config::parse(
            r#"
            profile = "mike"
            macros = ["ddssm.sty"]
            [profiles.mike]
            styled = "latex"
            [profiles.mike.style_commands]
            bfit = "\\bm"
            [profiles.mike.spellings]
            "ℝ" = "\\R"
            [profiles.ascii]
            greek = "unicode"
            "#,
            None,
        )
        .unwrap();
        let m = c.profile("mike").unwrap();
        assert_eq!((m.symbols, m.styled), (Form::Unicode, Form::Latex));
        assert_eq!(m.style_commands["bfit"], "\\bm");
        assert_eq!(m.style_commands["bf"], "\\mathbf");
        assert_eq!(m.spellings["ℝ"], "\\R");
        let a = c.profile("ascii").unwrap();
        assert_eq!((a.symbols, a.greek), (Form::Latex, Form::Unicode));
        assert_eq!(c.default_profile_name(), "mike");
    }

    #[test]
    fn notations_are_validated_and_resolved_per_profile() {
        let c = Config::parse(
            r#"
            [notations.KL]
            macro = "\\KL{#1}{#2}"
            expanded = "D_{\\mathrm{KL}}(#1 \\| #2)"
            [profiles.ascii.notations]
            KL = "macro"
            "#,
            None,
        )
        .unwrap();
        assert_eq!(c.profile("ascii").unwrap().notations["KL"], "\\KL{#1}{#2}");
        assert!(c.profile("unicode").unwrap().notations.is_empty());
        assert_eq!(c.notation_decls()[0].1.len(), 2);
        for bad in [
            "[notations.KL]\nmacro = \"\\\\KL{#1}{#3}\"",
            "[notations.KL]\na = \"\\\\KL{#1}{#2}\"\nb = \"\\\\KL{#1}\"",
            "[notations.KL]\na = \"#1 #2\"",
            "[profiles.ascii.notations]\nKL = \"macro\"",
            "[notations.KL]\nmacro = \"\\\\KL{#1}{#2}\"\n[profiles.ascii.notations]\nKL = \"short\"",
        ] {
            assert!(Config::parse(bad, None).is_err(), "{bad}");
        }
    }

    #[test]
    fn style_presets_and_latex_forms() {
        let c = Config::parse(
            r#"
            root = "main.tex"
            engine = "lualatex"
            [profiles.sym]
            styled = "latex"
            style_preset = "unicode-math"
            [profiles.sym.style_commands]
            bb = "\\mathbb"
            [profiles.plain]
            style_preset = "latex"
            "#,
            Some(Path::new("/paper")),
        )
        .unwrap();
        assert_eq!(c.root_path(), Some(PathBuf::from("/paper/main.tex")));
        assert_eq!(c.engine.as_deref(), Some("lualatex"));
        let sym = c.profile("sym").unwrap();
        assert_eq!(sym.preset, StylePreset::UnicodeMath);
        assert_eq!(c.profile("ascii").unwrap().preset, StylePreset::Latex);
        assert_eq!(sym.style_commands["bf"], "\\symbf");
        assert_eq!(sym.style_commands["bf_greek"], "\\symbf");
        assert_eq!(sym.style_commands["sfit"], "\\symsfit");
        assert_eq!(
            sym.style_commands["bb"], "\\mathbb",
            "its own override wins"
        );
        // A profile that says nothing takes the caller's preset; one that
        // says `latex` keeps it whatever the document loads.
        let ascii = c.profile_with("ascii", StylePreset::UnicodeMath).unwrap();
        assert_eq!(ascii.style_commands["bfit"], "\\symbfit");
        let plain = c.profile_with("plain", StylePreset::UnicodeMath).unwrap();
        assert_eq!(plain.style_commands["bfit"], "\\boldsymbol");
        assert_eq!(c.profile("ascii").unwrap().style_commands["bf"], "\\mathbf");
        let forced = c.profile("unicode").unwrap().latex_forms();
        assert_eq!(
            (forced.symbols, forced.styled, forced.greek),
            (Form::Latex, Form::Latex, Form::Latex)
        );
        assert!(Config::parse("root = \"\"", None).is_err());
        assert!(Config::parse("[profiles.a]\nstyle_preset = \"sym\"", None).is_err());
    }

    #[test]
    fn unknown_keys_are_errors() {
        assert!(Config::parse("profil = \"x\"", None).is_err());
        assert!(Config::parse("[profiles.a]\nbold = true", None).is_err());
        assert!(Config::parse("[profiles.a.style_commands]\nbold = \"\\\\x\"", None).is_err());
        assert!(Config::parse("version = 2", None).is_err());
    }
}
