//! `power-module` — AC and battery state, as a line of text on the terminal or
//! as JSON for a waybar custom module.

mod ac;
mod battery;
mod colour;
mod config;
mod render;
mod sysfs;
mod template;
#[cfg(test)]
mod testutil;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ac::State;
use colour::Colour;
use config::{Config, ColourWhen};
use render::{Reading, Scope, Waybar};
use sysfs::Error;

const HELP: &str = "\
power-module — AC and battery state, for the terminal or for waybar

Usage:
    power-module [options]

What to report (default: both):
        --ac              Only the AC adapter.
        --battery         Only the batteries.

Output:
    -w, --waybar          Emit one JSON object for a waybar custom module
                          (use with \"return-type\": \"json\").
    -f, --full            Add the supporting numbers under each line: charge,
                          health, rate, cycle count and any charge limit. The
                          waybar tooltip always carries these.
    -q, --quiet           Print nothing; exit 0 on external power, 1 on battery.
                          Unaffected by --ac / --battery.
        --color <WHEN>    Colour the text output: auto (default), always, never.
                          auto colours only when writing to a terminal. NO_COLOR
                          in the environment turns it off. JSON is never
                          coloured; style it with CSS instead.

Other:
    -a, --adapter <NAME>  Read this power supply as the AC adapter instead of
                          picking one, e.g. AC (see /sys/class/power_supply).
        --config <PATH>   Read this config file instead of searching for one.
        --no-config       Ignore any config file and use the built-in defaults.
    -h, --help            Show this help.
    -V, --version         Show the version.

Config:
    Optional. Searched for as power-module.toml in $XDG_CONFIG_HOME (default
    ~/.config) and then each of $XDG_CONFIG_DIRS (default /etc/xdg), either
    loose or in a power-module/ subdirectory. It sets the charge level
    thresholds, the terminal colours, the output formats, and defaults for the
    options above. Flags always win over the file.

Exit status:
    0  success (with --quiet: on external power)
    1  --quiet and running on battery
    2  the state could not be determined
";

/// What the command line asked for. Anything left `None` is decided by the
/// config file, and failing that by the built-in default.
struct Options {
    scope: Option<Scope>,
    full: bool,
    waybar: bool,
    quiet: bool,
    colour: Option<ColourWhen>,
    adapter: Option<String>,
    config: Option<PathBuf>,
    no_config: bool,
}

/// A bad command line, or `--help`/`--version`, ends the run before we ever
/// look at sysfs.
#[derive(Debug)]
enum Early {
    Message(String),
    Usage(String),
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Options, Early> {
    let (mut want_ac, mut want_battery) = (false, false);
    let mut options = Options {
        scope: None,
        full: false,
        waybar: false,
        quiet: false,
        colour: None,
        adapter: None,
        config: None,
        no_config: false,
    };

    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(Early::Message(HELP.to_string())),
            "-V" | "--version" => {
                return Err(Early::Message(format!(
                    "power-module {}\n",
                    env!("CARGO_PKG_VERSION")
                )));
            }
            "--ac" => want_ac = true,
            "--battery" => want_battery = true,
            "-f" | "--full" => options.full = true,
            "-w" | "--waybar" | "--json" => options.waybar = true,
            "-q" | "--quiet" => options.quiet = true,
            "-a" | "--adapter" => match args.next() {
                Some(name) => options.adapter = Some(name),
                None => return Err(Early::Usage(format!("{arg} needs an adapter name"))),
            },
            "--config" => match args.next() {
                Some(path) => options.config = Some(PathBuf::from(path)),
                None => return Err(Early::Usage(format!("{arg} needs a path"))),
            },
            "--no-config" => options.no_config = true,
            "--color" | "--colour" => match args.next().as_deref().map(ColourWhen::parse) {
                Some(Some(when)) => options.colour = Some(when),
                Some(None) => return Err(Early::Usage(format!("{arg} takes auto, always or never"))),
                None => return Err(Early::Usage(format!("{arg} needs a value"))),
            },
            _ => {
                if let Some(name) = arg.strip_prefix("--adapter=").filter(|n| !n.is_empty()) {
                    options.adapter = Some(name.to_string());
                } else if let Some(path) =
                    arg.strip_prefix("--config=").filter(|p| !p.is_empty())
                {
                    options.config = Some(PathBuf::from(path));
                } else if let Some(raw) =
                    arg.strip_prefix("--color=").or_else(|| arg.strip_prefix("--colour="))
                {
                    match ColourWhen::parse(raw) {
                        Some(when) => options.colour = Some(when),
                        None => {
                            return Err(Early::Usage(format!(
                                "--color takes auto, always or never, not {raw:?}"
                            )));
                        }
                    }
                } else {
                    return Err(Early::Usage(format!("unrecognised argument {arg:?}")));
                }
            }
        }
    }

    // Asking for both is the same as asking for neither.
    options.scope = match (want_ac, want_battery) {
        (true, false) => Some(Scope::Ac),
        (false, true) => Some(Scope::Battery),
        (true, true) => Some(Scope::All),
        (false, false) => None,
    };
    Ok(options)
}

/// The command line and the config file, reconciled. Flags win; the file fills
/// in what the flags left unsaid; the built-in defaults fill in the rest.
struct Settings {
    scope: Scope,
    adapter: Option<String>,
    colour: ColourWhen,
    config: Config,
}

fn resolve(options: &Options, config: Config) -> Settings {
    Settings {
        scope: options.scope.or(config.scope).unwrap_or(Scope::All),
        adapter: options.adapter.clone().or_else(|| config.adapter.clone()),
        colour: options.colour.or(config.colour).unwrap_or(ColourWhen::Auto),
        config,
    }
}

fn read(root: &Path, settings: &Settings) -> Result<Reading, Error> {
    let batteries = match settings.scope {
        Scope::Ac => Vec::new(),
        _ => battery::batteries(root)?,
    };

    let sources = match settings.scope {
        Scope::Battery => Vec::new(),
        Scope::Ac => ac::sources(root, settings.adapter.as_deref())?,
        // A laptop that publishes batteries but no mains supply is still worth
        // reporting on, so a missing adapter only sinks the run on its own.
        Scope::All => match ac::sources(root, settings.adapter.as_deref()) {
            Ok(sources) => sources,
            Err(Error::NoAcAdapter) if !batteries.is_empty() => Vec::new(),
            Err(error) => return Err(error),
        },
    };

    if settings.scope == Scope::Battery && batteries.is_empty() {
        return Err(Error::NoBattery);
    }
    Ok(Reading { sources, batteries })
}

/// `--quiet` answers one question — am I on the cord? — whatever the scope.
fn quiet_status(root: &Path, settings: &Settings) -> Result<State, Error> {
    let sources = ac::sources(root, settings.adapter.as_deref())?;
    Ok(ac::state_of(&sources))
}

fn main() -> ExitCode {
    let no_color = std::env::var("NO_COLOR").ok();
    // stdout and stderr are decided separately: one of the two is often a pipe
    // while the other is still the terminal.
    let shade_errors = |when: ColourWhen, text: &str| {
        let colour = when.resolve(std::io::stderr().is_terminal(), no_color.as_deref());
        Colour::RED.paint(text, colour)
    };
    let fail = |when: ColourWhen, message: String| {
        eprintln!("{}", shade_errors(when, &format!("power-module: {message}")));
        ExitCode::from(2)
    };

    let options = match parse_args(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(Early::Message(text)) => {
            print!("{text}");
            return ExitCode::SUCCESS;
        }
        Err(Early::Usage(problem)) => {
            // The colour flag may itself be what failed to parse, so this one
            // message falls back to the default.
            eprintln!("{}", shade_errors(ColourWhen::Auto, &format!("power-module: {problem}")));
            eprint!("{HELP}");
            return ExitCode::from(2);
        }
    };

    let loaded = if options.no_config {
        Ok((Config::default(), None))
    } else {
        config::load(options.config.as_deref())
    };
    let config = match loaded {
        Ok((config, _)) => config,
        // A broken config file is worth saying plainly even in waybar mode,
        // where it surfaces in the tooltip.
        Err(error) => {
            if options.waybar {
                println!("{}", Waybar::unknown(&error.to_string()).to_json());
                return ExitCode::SUCCESS;
            }
            return fail(options.colour.unwrap_or(ColourWhen::Auto), error.to_string());
        }
    };

    let settings = resolve(&options, config);
    let root = Path::new(sysfs::SYSFS_ROOT);
    let colour = settings
        .colour
        .resolve(std::io::stdout().is_terminal(), no_color.as_deref());

    if options.quiet {
        return match quiet_status(root, &settings) {
            Ok(State::Plugged) => ExitCode::SUCCESS,
            Ok(State::Unplugged) => ExitCode::FAILURE,
            Err(error) => fail(settings.colour, error.to_string()),
        };
    }

    let reading = match read(root, &settings) {
        Ok(reading) => reading,
        Err(error) => {
            // waybar keeps polling us forever, so hand it a well-formed object
            // it can style rather than a broken module.
            if options.waybar {
                println!("{}", Waybar::unknown(&error.to_string()).to_json());
                return ExitCode::SUCCESS;
            }
            return fail(settings.colour, error.to_string());
        }
    };

    if options.waybar {
        println!("{}", render::waybar(settings.scope, &reading, &settings.config).to_json());
    } else {
        println!(
            "{}",
            render::text(settings.scope, &reading, &settings.config, colour, options.full)
        );
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FakeRoot;

    fn parse(args: &[&str]) -> Result<Options, Early> {
        parse_args(args.iter().map(|arg| arg.to_string()))
    }

    /// The settings a bare command line lands on, with an optional config file.
    fn settings(args: &[&str], config: &str) -> Settings {
        let config = config::parse(config, Path::new("test.toml")).unwrap();
        resolve(&parse(args).unwrap(), config)
    }

    #[test]
    fn no_arguments_reports_everything_as_text() {
        let options = parse(&[]).unwrap();
        assert_eq!(options.scope, None);
        assert!(!options.waybar);
        assert_eq!(options.adapter, None);
        assert_eq!(settings(&[], "").scope, Scope::All);
    }

    #[test]
    fn scope_flags_select_one_half() {
        assert_eq!(parse(&["--ac"]).unwrap().scope, Some(Scope::Ac));
        assert_eq!(parse(&["--battery"]).unwrap().scope, Some(Scope::Battery));
        assert_eq!(parse(&["--ac", "--battery"]).unwrap().scope, Some(Scope::All));
    }

    #[test]
    fn adapter_takes_either_spelling() {
        assert_eq!(parse(&["-a", "AC"]).unwrap().adapter.as_deref(), Some("AC"));
        assert_eq!(parse(&["--adapter", "AC"]).unwrap().adapter.as_deref(), Some("AC"));
        assert_eq!(parse(&["--adapter=AC"]).unwrap().adapter.as_deref(), Some("AC"));
    }

    #[test]
    fn colour_takes_a_when_in_either_spelling() {
        assert_eq!(parse(&["--color", "never"]).unwrap().colour, Some(ColourWhen::Never));
        assert_eq!(parse(&["--color=always"]).unwrap().colour, Some(ColourWhen::Always));
        assert_eq!(parse(&["--colour=never"]).unwrap().colour, Some(ColourWhen::Never));
        assert_eq!(parse(&[]).unwrap().colour, None);
        assert!(matches!(parse(&["--color", "mauve"]), Err(Early::Usage(_))));
        assert!(matches!(parse(&["--color=mauve"]), Err(Early::Usage(_))));
        assert!(matches!(parse(&["--color"]), Err(Early::Usage(_))));
    }

    #[test]
    fn auto_follows_the_terminal_and_the_no_color_convention() {
        assert!(ColourWhen::Auto.resolve(true, None));
        assert!(!ColourWhen::Auto.resolve(false, None));
        assert!(!ColourWhen::Auto.resolve(true, Some("1")));
        // NO_COLOR set but empty is not an opt-out.
        assert!(ColourWhen::Auto.resolve(true, Some("")));
        // The explicit forms ignore both.
        assert!(ColourWhen::Always.resolve(false, Some("1")));
        assert!(!ColourWhen::Never.resolve(true, None));
    }

    #[test]
    fn a_config_path_takes_either_spelling() {
        assert_eq!(parse(&["--config", "a.toml"]).unwrap().config.unwrap().to_str(), Some("a.toml"));
        assert_eq!(parse(&["--config=a.toml"]).unwrap().config.unwrap().to_str(), Some("a.toml"));
        assert!(parse(&["--no-config"]).unwrap().no_config);
        assert!(matches!(parse(&["--config"]), Err(Early::Usage(_))));
    }

    #[test]
    fn a_dangling_adapter_flag_is_a_usage_error() {
        assert!(matches!(parse(&["--adapter"]), Err(Early::Usage(_))));
        assert!(matches!(parse(&["--frobnicate"]), Err(Early::Usage(_))));
    }

    #[test]
    fn the_config_file_supplies_what_the_flags_did_not() {
        let file = "[general]\nscope = \"battery\"\nadapter = \"AC\"\ncolor = \"never\"\n";
        let from_file = settings(&[], file);
        assert_eq!(from_file.scope, Scope::Battery);
        assert_eq!(from_file.adapter.as_deref(), Some("AC"));
        assert_eq!(from_file.colour, ColourWhen::Never);
    }

    #[test]
    fn flags_beat_the_config_file() {
        let file = "[general]\nscope = \"battery\"\nadapter = \"AC\"\ncolor = \"never\"\n";
        let overridden = settings(&["--ac", "--adapter", "BAT0", "--color=always"], file);
        assert_eq!(overridden.scope, Scope::Ac);
        assert_eq!(overridden.adapter.as_deref(), Some("BAT0"));
        assert_eq!(overridden.colour, ColourWhen::Always);
    }

    #[test]
    fn a_laptop_reads_end_to_end() {
        let root = FakeRoot::new(&[
            ("AC", &[("type", "Mains"), ("online", "0")]),
            ("BAT0", &[
                ("type", "Battery"),
                ("status", "Discharging"),
                ("present", "1"),
                ("capacity", "85"),
                ("energy_now", "33650000"),
                ("energy_full", "39370000"),
                ("power_now", "8434000"),
            ]),
        ]);
        let settings = settings(&[], "");
        let reading = read(root.path(), &settings).unwrap();
        assert_eq!(
            render::text(Scope::All, &reading, &settings.config, false, false),
            "AC: unplugged\nBAT0: discharging 85% (3h 59m remaining)"
        );
    }

    #[test]
    fn a_configured_format_and_palette_reach_the_output() {
        let root = FakeRoot::new(&[
            ("AC", &[("type", "Mains"), ("online", "0")]),
            ("BAT0", &[("type", "Battery"), ("status", "Discharging"), ("capacity", "50")]),
        ]);
        let file = "[levels]\nwarning = 60\n\n                    [colors]\nwarning = \"magenta\"\n\n                    [formats]\nbattery = \"{name} {percent}% {level}\"\n";
        let settings = settings(&["--battery"], file);
        let reading = read(root.path(), &settings).unwrap();
        assert_eq!(
            render::text(Scope::Battery, &reading, &settings.config, true, false),
            "\x1b[35mBAT0 50% warning\x1b[0m"
        );
    }

    #[test]
    fn asking_only_for_a_battery_that_is_not_there_is_an_error() {
        let root = FakeRoot::new(&[("AC", &[("type", "Mains"), ("online", "1")])]);
        assert!(matches!(read(root.path(), &settings(&["--battery"], "")), Err(Error::NoBattery)));
        // ...but the default scope still reports the cord on such a machine.
        let default = settings(&[], "");
        let reading = read(root.path(), &default).unwrap();
        assert_eq!(render::text(Scope::All, &reading, &default.config, false, false), "AC: plugged in");
    }

    #[test]
    fn a_machine_with_batteries_but_no_mains_supply_still_reports() {
        let root = FakeRoot::new(&[(
            "BAT0",
            &[("type", "Battery"), ("status", "Discharging"), ("capacity", "50")],
        )]);
        let default = settings(&[], "");
        let reading = read(root.path(), &default).unwrap();
        assert_eq!(
            render::text(Scope::All, &reading, &default.config, false, false),
            "BAT0: discharging 50%"
        );
        // Asking for the adapter alone on that machine is still an error.
        assert!(matches!(read(root.path(), &settings(&["--ac"], "")), Err(Error::NoAcAdapter)));
    }
}
