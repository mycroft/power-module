//! The `power-module.toml` config file.
//!
//! Everything here has a working default, so the file is entirely optional and
//! a machine without one behaves exactly as it did before the file existed.
//! Command-line flags always win over the file.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use toml::Value;

use crate::battery::Levels;
use crate::colour::Colour;
use crate::render::Scope;
use crate::template::Template;

/// The file looked for in each XDG config directory.
pub const FILE_NAME: &str = "power-module.toml";

/// When to emit ANSI colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColourWhen {
    Auto,
    Always,
    Never,
}

impl ColourWhen {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(ColourWhen::Auto),
            "always" | "yes" | "force" => Some(ColourWhen::Always),
            "never" | "no" | "none" => Some(ColourWhen::Never),
            _ => None,
        }
    }

    /// `auto` means a terminal is reading it and the user has not opted out via
    /// the NO_COLOR convention.
    pub fn resolve(self, is_terminal: bool, no_color: Option<&str>) -> bool {
        match self {
            ColourWhen::Always => true,
            ColourWhen::Never => false,
            ColourWhen::Auto => {
                is_terminal && !matches!(no_color, Some(value) if !value.is_empty())
            }
        }
    }
}

fn parse_scope(raw: &str) -> Option<Scope> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ac" | "adapter" => Some(Scope::Ac),
        "battery" => Some(Scope::Battery),
        "all" | "both" => Some(Scope::All),
        _ => None,
    }
}

/// Which colour each state is drawn in on the terminal. Waybar takes its
/// colours from CSS instead, keyed off the classes the JSON carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub plugged: Colour,
    pub unplugged: Colour,
    pub charging: Colour,
    pub discharging: Colour,
    pub full: Colour,
    pub not_charging: Colour,
    pub warning: Colour,
    pub critical: Colour,
    /// Low but recovering — worth less alarm than low and draining.
    pub critical_charging: Colour,
    pub unknown: Colour,
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            plugged: Colour::GREEN,
            unplugged: Colour::YELLOW,
            charging: Colour::GREEN,
            discharging: Colour::DEFAULT,
            full: Colour::GREEN,
            not_charging: Colour::DEFAULT,
            warning: Colour::YELLOW,
            critical: Colour::RED,
            critical_charging: Colour::YELLOW,
            unknown: Colour::RED,
        }
    }
}

pub const AC_FIELDS: &[&str] = &["name", "state"];
pub const BATTERY_FIELDS: &[&str] =
    &["name", "status", "percent", "time", "caption", "level"];
pub const SUMMARY_FIELDS: &[&str] = &["status", "percent", "time", "caption", "level"];

const DEFAULT_AC: &str = "{name}: {state}";
const DEFAULT_BATTERY: &str = "{name}: {status}[ {percent}%][ ({time} {caption})]";
const DEFAULT_SUMMARY: &str = "Battery: {status}[ {percent}%][ ({time} {caption})]";
const DEFAULT_BAR: &str = "[{percent}%][ {time}]";
const DEFAULT_BAR_AC: &str = "{name}";

/// The output lines, as templates. See [`crate::template`] for the syntax.
#[derive(Debug, Clone, PartialEq)]
pub struct Formats {
    /// The adapter line of the text report.
    pub ac: Template,
    /// One battery's line of the text report.
    pub battery: Template,
    /// The combined line shown above several batteries.
    pub summary: Template,
    /// The waybar `text` field for the battery.
    pub bar: Template,
    /// The waybar `text` field when reporting the adapter alone.
    pub bar_ac: Template,
}

impl Default for Formats {
    fn default() -> Self {
        let build = |source: &str, fields: &[&str]| {
            Template::parse(source, fields).expect("built-in formats are valid")
        };
        Formats {
            ac: build(DEFAULT_AC, AC_FIELDS),
            battery: build(DEFAULT_BATTERY, BATTERY_FIELDS),
            summary: build(DEFAULT_SUMMARY, SUMMARY_FIELDS),
            bar: build(DEFAULT_BAR, SUMMARY_FIELDS),
            bar_ac: build(DEFAULT_BAR_AC, AC_FIELDS),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Config {
    /// `None` where the file said nothing, so a flag or the built-in default
    /// decides instead.
    pub scope: Option<Scope>,
    pub adapter: Option<String>,
    pub colour: Option<ColourWhen>,
    pub levels: Levels,
    pub palette: Palette,
    pub formats: Formats,
}

#[derive(Debug)]
pub enum Error {
    Read { path: PathBuf, source: io::Error },
    Parse { path: PathBuf, message: String },
    Invalid { path: PathBuf, key: String, problem: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Read { path, source } => {
                write!(f, "cannot read config {}: {source}", path.display())
            }
            Error::Parse { path, message } => write!(f, "{}: {message}", path.display()),
            Error::Invalid { path, key, problem } => {
                write!(f, "{}: {key}: {problem}", path.display())
            }
        }
    }
}

/// The files consulted, in order; the first that exists is the one used.
///
/// Follows the XDG basedir spec: `$XDG_CONFIG_HOME` (default `~/.config`) then
/// each of `$XDG_CONFIG_DIRS` (default `/etc/xdg`). In each, both a bare
/// `power-module.toml` and a `power-module/power-module.toml` are accepted, so
/// the file can sit loose or in its own directory.
pub fn search_paths() -> Vec<PathBuf> {
    let non_empty = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());

    let home_config = non_empty("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| non_empty("HOME").map(|home| PathBuf::from(home).join(".config")));

    let system_config = non_empty("XDG_CONFIG_DIRS")
        .map(|dirs| {
            std::env::split_paths(&dirs).filter(|dir| !dir.as_os_str().is_empty()).collect()
        })
        .unwrap_or_else(|| vec![PathBuf::from("/etc/xdg")]);

    home_config
        .into_iter()
        .chain(system_config)
        // The spec says relative paths in these variables must be ignored.
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| [dir.join("power-module").join(FILE_NAME), dir.join(FILE_NAME)])
        .collect()
}

/// Load the config, returning it alongside the file it came from.
///
/// With no `explicit` path and no file anywhere on the search path, this is the
/// built-in defaults and `None`. An explicit path that does not exist is an
/// error — the user asked for that file by name.
pub fn load(explicit: Option<&Path>) -> Result<(Config, Option<PathBuf>), Error> {
    let found = match explicit {
        Some(path) => Some(path.to_path_buf()),
        None => search_paths().into_iter().find(|path| path.is_file()),
    };
    let Some(path) = found else {
        return Ok((Config::default(), None));
    };
    let text = fs::read_to_string(&path)
        .map_err(|source| Error::Read { path: path.clone(), source })?;
    let config = parse(&text, &path)?;
    Ok((config, Some(path)))
}

/// One section of the file, or `None` if it is absent. `names` lists the
/// accepted spellings; the first is the canonical one.
fn section<'a>(
    root: &'a toml::Table,
    path: &Path,
    names: &[&str],
) -> Result<Option<&'a toml::Table>, Error> {
    let mut found = None;
    for name in names {
        let Some(value) = root.get(*name) else { continue };
        if found.is_some() {
            return Err(Error::Invalid {
                path: path.to_path_buf(),
                key: format!("[{}]", names[0]),
                problem: format!("also spelled [{name}]; use one or the other"),
            });
        }
        found = Some(value.as_table().ok_or_else(|| Error::Invalid {
            path: path.to_path_buf(),
            key: format!("[{name}]"),
            problem: format!("should be a table, not {}", value.type_str()),
        })?);
    }
    Ok(found)
}

fn reject_unknown(
    table: &toml::Table,
    section: &str,
    known: &[&str],
    path: &Path,
) -> Result<(), Error> {
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            return Err(Error::Invalid {
                path: path.to_path_buf(),
                key: format!("{section}.{key}"),
                problem: format!("unknown setting; this section takes {}", known.join(", ")),
            });
        }
    }
    Ok(())
}

/// Look a key up under any of its accepted spellings.
fn value<'a>(table: &'a toml::Table, names: &'a [&'a str]) -> Option<(&'a str, &'a Value)> {
    names.iter().find_map(|name| table.get(*name).map(|value| (*name, value)))
}

fn as_string(
    table: &toml::Table,
    section: &str,
    names: &[&str],
    path: &Path,
) -> Result<Option<String>, Error> {
    let Some((name, value)) = value(table, names) else { return Ok(None) };
    value.as_str().map(|text| Some(text.to_string())).ok_or_else(|| Error::Invalid {
        path: path.to_path_buf(),
        key: format!("{section}.{name}"),
        problem: format!("should be a string, not {}", value.type_str()),
    })
}

/// A percentage, written as either an integer or a float.
fn as_percent(
    table: &toml::Table,
    section: &str,
    name: &str,
    path: &Path,
) -> Result<Option<f64>, Error> {
    let Some(value) = table.get(name) else { return Ok(None) };
    let invalid = |problem: String| Error::Invalid {
        path: path.to_path_buf(),
        key: format!("{section}.{name}"),
        problem,
    };
    let number = value
        .as_integer()
        .map(|integer| integer as f64)
        .or_else(|| value.as_float())
        .ok_or_else(|| invalid(format!("should be a number, not {}", value.type_str())))?;
    if !(0.0..=100.0).contains(&number) {
        return Err(invalid(format!("should be a percentage between 0 and 100, not {number}")));
    }
    Ok(Some(number))
}

fn as_colour(
    table: &toml::Table,
    names: &[&str],
    path: &Path,
) -> Result<Option<Colour>, Error> {
    let Some((name, value)) = value(table, names) else { return Ok(None) };
    let invalid = |problem: String| Error::Invalid {
        path: path.to_path_buf(),
        key: format!("colors.{name}"),
        problem,
    };
    let text = value
        .as_str()
        .ok_or_else(|| invalid(format!("should be a string, not {}", value.type_str())))?;
    Colour::parse(text)
        .map(Some)
        .ok_or_else(|| invalid(format!("{text:?} is not a colour; try {}", Colour::NAMES)))
}

fn as_template(
    table: &toml::Table,
    name: &str,
    fields: &[&str],
    path: &Path,
) -> Result<Option<Template>, Error> {
    let Some(value) = table.get(name) else { return Ok(None) };
    let invalid = |problem: String| Error::Invalid {
        path: path.to_path_buf(),
        key: format!("formats.{name}"),
        problem,
    };
    let text = value
        .as_str()
        .ok_or_else(|| invalid(format!("should be a string, not {}", value.type_str())))?;
    Template::parse(text, fields).map(Some).map_err(invalid)
}

pub fn parse(text: &str, path: &Path) -> Result<Config, Error> {
    let root: Value = toml::from_str(text)
        .map_err(|error| Error::Parse { path: path.to_path_buf(), message: error.to_string() })?;
    let root = root.as_table().ok_or_else(|| Error::Parse {
        path: path.to_path_buf(),
        message: "expected a table of settings".to_string(),
    })?;

    for key in root.keys() {
        if !["general", "levels", "colors", "colours", "formats", "format"]
            .contains(&key.as_str())
        {
            return Err(Error::Invalid {
                path: path.to_path_buf(),
                key: format!("[{key}]"),
                problem: "unknown section; expected general, levels, colors or formats"
                    .to_string(),
            });
        }
    }

    let mut config = Config::default();

    if let Some(table) = section(root, path, &["general"])? {
        let name = "general";
        reject_unknown(table, name, &["scope", "adapter", "color", "colour"], path)?;
        if let Some(raw) = as_string(table, name, &["scope"], path)? {
            config.scope = Some(parse_scope(&raw).ok_or_else(|| Error::Invalid {
                path: path.to_path_buf(),
                key: "general.scope".to_string(),
                problem: format!("{raw:?} is not a scope; try ac, battery or all"),
            })?);
        }
        config.adapter = as_string(table, name, &["adapter"], path)?;
        if let Some(raw) = as_string(table, name, &["color", "colour"], path)? {
            config.colour = Some(ColourWhen::parse(&raw).ok_or_else(|| Error::Invalid {
                path: path.to_path_buf(),
                key: "general.color".to_string(),
                problem: format!("{raw:?} is not a setting; try auto, always or never"),
            })?);
        }
    }

    if let Some(table) = section(root, path, &["levels"])? {
        let name = "levels";
        reject_unknown(table, name, &["full", "warning", "critical"], path)?;
        let levels = &mut config.levels;
        if let Some(full) = as_percent(table, name, "full", path)? {
            levels.full = full;
        }
        if let Some(warning) = as_percent(table, name, "warning", path)? {
            levels.warning = warning;
        }
        if let Some(critical) = as_percent(table, name, "critical", path)? {
            levels.critical = critical;
        }
        // An out-of-order set of thresholds would silently make one band
        // unreachable, so say so instead.
        if levels.critical > levels.warning {
            return Err(Error::Invalid {
                path: path.to_path_buf(),
                key: "levels.critical".to_string(),
                problem: format!(
                    "is above levels.warning ({} > {}), which leaves no warning band",
                    levels.critical, levels.warning
                ),
            });
        }
        if levels.warning >= levels.full {
            return Err(Error::Invalid {
                path: path.to_path_buf(),
                key: "levels.warning".to_string(),
                problem: format!(
                    "is not below levels.full ({} >= {}), which leaves no good band",
                    levels.warning, levels.full
                ),
            });
        }
    }

    if let Some(table) = section(root, path, &["colors", "colours"])? {
        let name = "colors";
        let known = [
            "plugged",
            "unplugged",
            "charging",
            "discharging",
            "full",
            "not_charging",
            "warning",
            "critical",
            "critical_charging",
            "unknown",
        ];
        reject_unknown(table, name, &known, path)?;
        let palette = &mut config.palette;
        for (names, slot) in [
            (&["plugged"][..], &mut palette.plugged),
            (&["unplugged"][..], &mut palette.unplugged),
            (&["charging"][..], &mut palette.charging),
            (&["discharging"][..], &mut palette.discharging),
            (&["full"][..], &mut palette.full),
            (&["not_charging"][..], &mut palette.not_charging),
            (&["warning"][..], &mut palette.warning),
            (&["critical"][..], &mut palette.critical),
            (&["critical_charging"][..], &mut palette.critical_charging),
            (&["unknown"][..], &mut palette.unknown),
        ] {
            if let Some(colour) = as_colour(table, names, path)? {
                *slot = colour;
            }
        }
    }

    if let Some(table) = section(root, path, &["formats", "format"])? {
        let name = "formats";
        reject_unknown(table, name, &["ac", "battery", "summary", "bar", "bar_ac"], path)?;
        let formats = &mut config.formats;
        for (key, fields, slot) in [
            ("ac", AC_FIELDS, &mut formats.ac),
            ("battery", BATTERY_FIELDS, &mut formats.battery),
            ("summary", SUMMARY_FIELDS, &mut formats.summary),
            ("bar", SUMMARY_FIELDS, &mut formats.bar),
            ("bar_ac", AC_FIELDS, &mut formats.bar_ac),
        ] {
            if let Some(template) = as_template(table, key, fields, path)? {
                *slot = template;
            }
        }
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(text: &str) -> Result<Config, Error> {
        parse(text, Path::new("power-module.toml"))
    }

    fn problem(text: &str) -> String {
        parse_str(text).unwrap_err().to_string()
    }

    #[test]
    fn an_empty_file_is_the_built_in_defaults() {
        assert_eq!(parse_str("").unwrap(), Config::default());
        assert_eq!(parse_str("# just a comment\n").unwrap(), Config::default());
    }

    #[test]
    fn general_settings_are_optional_one_by_one() {
        let config = parse_str("[general]\nscope = \"battery\"\n").unwrap();
        assert_eq!(config.scope, Some(Scope::Battery));
        // Untouched settings stay unset, so a flag or the default decides.
        assert_eq!(config.adapter, None);
        assert_eq!(config.colour, None);
    }

    #[test]
    fn levels_accept_integers_and_floats() {
        let config = parse_str("[levels]\nwarning = 40\ncritical = 12.5\n").unwrap();
        assert_eq!(config.levels.warning, 40.0);
        assert_eq!(config.levels.critical, 12.5);
        // `full` was not mentioned, so it keeps its default.
        assert_eq!(config.levels.full, Levels::default().full);
    }

    #[test]
    fn thresholds_that_would_hide_a_band_are_rejected() {
        let error = problem("[levels]\nwarning = 10\ncritical = 20\n");
        assert!(error.contains("levels.critical"), "{error}");
        assert!(error.contains("no warning band"), "{error}");

        let error = problem("[levels]\nwarning = 99\n");
        assert!(error.contains("no good band"), "{error}");
    }

    #[test]
    fn a_percentage_outside_the_range_is_rejected() {
        let error = problem("[levels]\nwarning = 130\n");
        assert!(error.contains("between 0 and 100"), "{error}");
        assert!(problem("[levels]\nwarning = -1\n").contains("between 0 and 100"));
    }

    #[test]
    fn colours_are_named_and_the_section_takes_both_spellings() {
        let config = parse_str("[colors]\ndischarging = \"cyan\"\n").unwrap();
        assert_eq!(config.palette.discharging, Colour::CYAN);
        let config = parse_str("[colours]\ndischarging = \"bright-blue\"\n").unwrap();
        assert_eq!(config.palette.discharging, Colour::parse("bright-blue").unwrap());
        // Untouched entries keep their defaults.
        assert_eq!(config.palette.plugged, Colour::GREEN);
    }

    #[test]
    fn the_same_section_spelled_two_ways_is_an_error() {
        let error = problem("[colors]\nplugged = \"red\"\n\n[colours]\nplugged = \"blue\"\n");
        assert!(error.contains("use one or the other"), "{error}");
    }

    #[test]
    fn an_unknown_colour_names_the_alternatives() {
        let error = problem("[colors]\nplugged = \"mauve\"\n");
        assert!(error.contains("colors.plugged"), "{error}");
        assert!(error.contains("not a colour"), "{error}");
        assert!(error.contains("yellow"), "{error}");
    }

    #[test]
    fn formats_are_validated_when_the_file_loads_not_when_it_prints() {
        let config = parse_str("[formats]\nbattery = \"{name} {percent}%\"\n").unwrap();
        assert_ne!(config.formats.battery, Formats::default().battery);

        let error = problem("[formats]\nbattery = \"{name} {health}\"\n");
        assert!(error.contains("formats.battery"), "{error}");
        assert!(error.contains("unknown placeholder {health}"), "{error}");

        // The adapter line has no battery fields to offer.
        assert!(problem("[formats]\nac = \"{percent}\"\n").contains("unknown placeholder"));

        // Malformed templates are caught too.
        assert!(problem("[formats]\nbattery = \"{name\"\n").contains("unmatched"));
    }

    #[test]
    fn a_typo_in_a_key_is_reported_rather_than_ignored() {
        let error = problem("[levels]\ncritcal = 15\n");
        assert!(error.contains("levels.critcal"), "{error}");
        assert!(error.contains("unknown setting"), "{error}");
        assert!(error.contains("critical"), "{error}");

        let error = problem("[battery]\nwarning = 10\n");
        assert!(error.contains("unknown section"), "{error}");
    }

    #[test]
    fn a_setting_of_the_wrong_type_says_which_type_it_got() {
        let error = problem("[general]\nadapter = 3\n");
        assert!(error.contains("should be a string, not integer"), "{error}");
        let error = problem("[levels]\nwarning = \"lots\"\n");
        assert!(error.contains("should be a number, not string"), "{error}");
        let error = problem("[general]\nscope = \"sideways\"\n");
        assert!(error.contains("not a scope"), "{error}");
    }

    #[test]
    fn a_toml_syntax_error_carries_its_line() {
        let error = problem("[levels\nwarning = 30\n");
        assert!(error.contains("power-module.toml"), "{error}");
        assert!(error.contains("line 1"), "{error}");
    }

    #[test]
    fn the_search_path_follows_the_xdg_spec() {
        // SAFETY: single-threaded test, and every variable read here is set.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", "/home/someone/.config");
            std::env::set_var("XDG_CONFIG_DIRS", "/etc/xdg:relative/ignored:/other/xdg");
        }
        let paths: Vec<String> =
            search_paths().iter().map(|path| path.display().to_string()).collect();
        assert_eq!(
            paths,
            [
                "/home/someone/.config/power-module/power-module.toml",
                "/home/someone/.config/power-module.toml",
                "/etc/xdg/power-module/power-module.toml",
                "/etc/xdg/power-module.toml",
                "/other/xdg/power-module/power-module.toml",
                "/other/xdg/power-module.toml",
            ]
        );
    }

    #[test]
    fn a_named_config_file_that_is_missing_is_an_error() {
        let missing = Path::new("/nonexistent/power-module.toml");
        assert!(matches!(load(Some(missing)), Err(Error::Read { .. })));
    }
}
