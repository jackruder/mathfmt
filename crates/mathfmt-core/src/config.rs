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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    pub symbols: Option<Form>,
    pub styled: Option<Form>,
    pub greek: Option<Form>,
    pub inline_macros: Option<bool>,
    pub script_reads_as: Option<String>,
    #[serde(default)]
    pub style_commands: BTreeMap<String, String>,
    #[serde(default)]
    pub spellings: BTreeMap<String, String>,
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
            math_environments: Vec::new(),
            verbatim_environments: Vec::new(),
            text_commands: Vec::new(),
            profiles: BTreeMap::new(),
            symbols: BTreeMap::new(),
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
    /// style name (or `bf_greek`, `bb_digit`) → command, possibly with `#1`.
    pub style_commands: HashMap<String, String>,
    /// atom key or character → spelling.
    pub spellings: HashMap<String, String>,
}

const DEFAULT_STYLE_COMMANDS: &[(&str, &str)] = &[
    ("up", "\\mathrm"),
    ("it", "\\mathit"),
    ("bf", "\\mathbf"),
    ("bf_greek", "\\boldsymbol"),
    ("bfit", "\\boldsymbol"),
    ("bb", "\\mathbb"),
    ("bb_digit", "\\mathbbm"),
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

const STYLE_COMMAND_KEYS: &[&str] = &["bf_greek", "bb_digit"];

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
        }
        Ok(())
    }

    /// Resolve a profile by name, with the built-in `unicode` and `ascii` as
    /// bases. A profile the file defines under another name starts from the
    /// `unicode` defaults.
    pub fn profile(&self, name: &str) -> Result<Profile, ConfigError> {
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
        let mut style_commands: HashMap<String, String> = DEFAULT_STYLE_COMMANDS
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
            style_commands,
            spellings: user.spellings.into_iter().collect(),
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
    fn unknown_keys_are_errors() {
        assert!(Config::parse("profil = \"x\"", None).is_err());
        assert!(Config::parse("[profiles.a]\nbold = true", None).is_err());
        assert!(Config::parse("[profiles.a.style_commands]\nbold = \"\\\\x\"", None).is_err());
        assert!(Config::parse("version = 2", None).is_err());
    }
}
