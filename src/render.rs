//! Turning readings into the two things this tool prints: coloured lines for a
//! terminal, and one JSON object for waybar.
//!
//! Both are driven by the templates and palette in [`crate::config`], so the
//! shape and the colour of every line is the user's to change.

use std::time::Duration;

use crate::ac::{self, Source, State};
use crate::battery::{self, Battery, Level, Levels, Status};
use crate::colour::Colour;
use crate::config::{Config, Palette};
use crate::template::Template;

/// What the caller asked to see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Ac,
    Battery,
    All,
}

/// Everything read from sysfs for one run.
pub struct Reading {
    pub sources: Vec<Source>,
    pub batteries: Vec<Battery>,
}

/// Green on the cord, yellow off it, by default.
fn ac_colour(state: State, palette: &Palette) -> Colour {
    match state {
        State::Plugged => palette.plugged,
        State::Unplugged => palette.unplugged,
    }
}

/// How alarmed to be about a battery.
///
/// A low battery is the one thing the AC indicator cannot tell you, so the
/// level speaks first and the status only decides the quiet cases. Charging out
/// of a critical level has its own colour: it is recovering, but you still
/// cannot walk away with it.
pub fn battery_colour(
    status: Status,
    percent: Option<f64>,
    levels: &Levels,
    palette: &Palette,
) -> Colour {
    if status == Status::Unknown {
        return palette.unknown;
    }
    let calm = match status {
        Status::Charging => palette.charging,
        Status::Full => palette.full,
        Status::NotCharging => palette.not_charging,
        _ => palette.discharging,
    };
    match percent.map(|percent| levels.of(percent)) {
        Some(Level::Critical) if status == Status::Charging => palette.critical_charging,
        Some(Level::Critical) => palette.critical,
        Some(Level::Warning) => palette.warning,
        _ => calm,
    }
}

/// Minutes, not seconds: a runtime estimate that ticks every second would be
/// false precision, and it changes the moment the load does.
///
/// Sub-minute spans round down to `0m` rather than reading `<1m`: waybar renders
/// module text as Pango markup, where a bare `<` starts a tag and breaks the
/// whole label.
pub fn format_duration(duration: Duration) -> String {
    let total_minutes = (duration.as_secs() + 30) / 60;
    let (hours, minutes) = (total_minutes / 60, total_minutes % 60);
    match hours {
        0 => format!("{minutes}m"),
        _ => format!("{hours}h {minutes:02}m"),
    }
}

/// Escape the three characters Pango treats as markup.
///
/// Used only for text this program generates about its own failures. Templates
/// from the config file are left alone on purpose, so a format string may carry
/// `<span>` the way waybar's own modules do.
fn pango_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn percent_field(percent: Option<f64>) -> Option<String> {
    percent.map(|percent| format!("{}", percent.round() as i64))
}

/// The fields a battery line or the bar text is rendered against. `caption`
/// follows `time`: with no runtime to show there is nothing to caption.
fn battery_fields(
    name: Option<&str>,
    status: Status,
    percent: Option<f64>,
    remaining: Option<Duration>,
    levels: &Levels,
) -> Vec<(&'static str, Option<String>)> {
    let mut fields = vec![
        ("status", Some(status.describe().to_string())),
        ("percent", percent_field(percent)),
        ("time", remaining.map(format_duration)),
        ("caption", remaining.map(|_| status.time_caption().to_string())),
        ("level", percent.map(|percent| levels.of(percent).as_str().to_string())),
    ];
    if let Some(name) = name {
        fields.push(("name", Some(name.to_string())));
    }
    fields
}

fn ac_fields(sources: &[Source], state: State) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("name", Some(ac::label(sources))),
        ("state", Some(state.describe().to_string())),
    ]
}

/// One `(line, colour)` per thing worth reporting. Several batteries get a
/// combined line first, then one indented line each.
fn lines(scope: Scope, reading: &Reading, config: &Config) -> Vec<(String, Colour)> {
    let mut lines = Vec::new();
    let (levels, palette) = (&config.levels, &config.palette);

    if scope != Scope::Battery && !reading.sources.is_empty() {
        let state = ac::state_of(&reading.sources);
        lines.push((
            config.formats.ac.render(&ac_fields(&reading.sources, state)),
            ac_colour(state, palette),
        ));
    }

    if scope != Scope::Ac {
        let entry_line = |entry: &Battery| {
            let fields = battery_fields(
                Some(&entry.name),
                entry.status,
                entry.percent,
                battery::remaining(entry),
                levels,
            );
            (
                config.formats.battery.render(&fields),
                battery_colour(entry.status, entry.percent, levels, palette),
            )
        };

        if reading.batteries.len() > 1 {
            if let Some(summary) = battery::summarise(&reading.batteries) {
                let fields = battery_fields(
                    None,
                    summary.status,
                    summary.percent,
                    summary.remaining,
                    levels,
                );
                lines.push((
                    config.formats.summary.render(&fields),
                    battery_colour(summary.status, summary.percent, levels, palette),
                ));
            }
            for entry in &reading.batteries {
                let (line, colour) = entry_line(entry);
                lines.push((format!("  {line}"), colour));
            }
        } else {
            lines.extend(reading.batteries.iter().map(entry_line));
        }
    }

    lines
}

/// The lines a human reads.
pub fn text(scope: Scope, reading: &Reading, config: &Config, colour: bool) -> String {
    lines(scope, reading, config)
        .iter()
        .map(|(line, shade)| shade.paint(line, colour))
        .collect::<Vec<_>>()
        .join("\n")
}

fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The object waybar reads. `alt` drives `format-icons`, `class` drives CSS,
/// and `percentage` is what `format` interpolates as `{percentage}`.
pub struct Waybar {
    pub text: String,
    pub alt: String,
    /// Several classes, so CSS can combine them the way waybar's own battery
    /// module does: `#custom-battery.discharging.critical`.
    pub classes: Vec<String>,
    pub tooltip: String,
    pub percentage: Option<f64>,
}

impl Waybar {
    pub fn to_json(&self) -> String {
        let classes = self
            .classes
            .iter()
            .map(|class| format!(r#""{}""#, json_escape(class)))
            .collect::<Vec<_>>()
            .join(",");
        let percentage = match self.percentage {
            Some(percentage) => format!(r#","percentage":{}"#, percentage.round() as i64),
            None => String::new(),
        };
        format!(
            r#"{{"text":"{}","alt":"{}","class":[{}],"tooltip":"{}"{}}}"#,
            json_escape(&self.text),
            json_escape(&self.alt),
            classes,
            json_escape(&self.tooltip),
            percentage
        )
    }

    /// Used when nothing could be read, so waybar still gets a well-formed
    /// object it can style rather than a broken module.
    pub fn unknown(reason: &str) -> Self {
        Waybar {
            text: "?".to_string(),
            alt: "unknown".to_string(),
            classes: vec!["unknown".to_string()],
            // A config error quotes the user's own file back at them, which may
            // well contain markup characters.
            tooltip: pango_escape(&format!("power-module: {reason}")),
            percentage: None,
        }
    }
}

/// Status, then charge level, then — in the combined module — the cord, with
/// duplicates dropped so `full` never lands twice.
fn classes(tokens: &[&str]) -> Vec<String> {
    let mut classes: Vec<String> = Vec::new();
    for token in tokens {
        if !classes.iter().any(|held| held == token) {
            classes.push(token.to_string());
        }
    }
    classes
}

fn render_or(template: &Template, fields: &[(&str, Option<String>)], fallback: &str) -> String {
    let text = template.render(fields);
    // A template made entirely of optional groups can render to nothing; say
    // something rather than showing an empty module.
    if text.is_empty() { fallback.to_string() } else { text }
}

pub fn waybar(scope: Scope, reading: &Reading, config: &Config) -> Waybar {
    // The tooltip is styled by CSS, never by escape codes.
    let tooltip = text(scope, reading, config, false);
    let summary = battery::summarise(&reading.batteries);

    // AC-only, or a machine with no battery at all: report the cord.
    if scope == Scope::Ac || summary.is_none() {
        let state = ac::state_of(&reading.sources);
        let fields = ac_fields(&reading.sources, state);
        return Waybar {
            text: render_or(&config.formats.bar_ac, &fields, state.describe()),
            alt: state.as_str().to_string(),
            classes: classes(&[state.as_str()]),
            tooltip,
            percentage: None,
        };
    }

    let summary = summary.expect("checked above");
    let level = summary.percent.map(|percent| config.levels.of(percent));
    let mut tokens = vec![summary.status.as_str()];
    if let Some(level) = level {
        tokens.push(level.as_str());
    }
    // The combined module shows the battery, but the cord is worth styling too.
    if scope == Scope::All && !reading.sources.is_empty() {
        tokens.push(ac::state_of(&reading.sources).as_str());
    }

    let fields = battery_fields(
        None,
        summary.status,
        summary.percent,
        summary.remaining,
        &config.levels,
    );
    Waybar {
        text: render_or(&config.formats.bar, &fields, summary.status.describe()),
        alt: summary.status.as_str().to_string(),
        classes: classes(&tokens),
        tooltip,
        percentage: summary.percent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery::Unit;
    use crate::config::Config;

    fn ac_source(name: &str, state: State) -> Source {
        Source { name: name.to_string(), kind: "Mains".to_string(), state }
    }

    fn bat(name: &str, status: Status, percent: f64, hours: Option<f64>) -> Battery {
        // 100 Wh cell, so `now` follows the percentage directly.
        let full = 100_000_000.0;
        let now = full * percent / 100.0;
        Battery {
            name: name.to_string(),
            status,
            percent: Some(percent),
            unit: Some(Unit::Energy),
            now: Some(now),
            full: Some(full),
            rate: hours.map(|hours| match status {
                Status::Charging => (full - now) / hours,
                _ => now / hours,
            }),
        }
    }

    fn laptop(state: State, battery: Battery) -> Reading {
        Reading { sources: vec![ac_source("AC", state)], batteries: vec![battery] }
    }

    #[test]
    fn durations_round_to_the_minute() {
        assert_eq!(format_duration(Duration::from_secs(90)), "2m");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h 00m");
        assert_eq!(format_duration(Duration::from_secs(14_340)), "3h 59m");
    }

    #[test]
    fn a_sub_minute_span_never_emits_a_pango_tag() {
        // waybar parses module text as markup, so "<1m" would break the label.
        assert_eq!(format_duration(Duration::from_secs(0)), "0m");
        assert_eq!(format_duration(Duration::from_secs(29)), "0m");
        for seconds in [0, 1, 29, 30, 59, 60, 3599, 86_400] {
            let rendered = format_duration(Duration::from_secs(seconds));
            assert!(!rendered.contains(['<', '>', '&']), "{seconds}s -> {rendered:?}");
        }
    }

    #[test]
    fn an_error_tooltip_escapes_markup_it_is_quoting() {
        let out = Waybar::unknown("formats.bar: unknown placeholder in \"<b>{x}</b> & co\"");
        assert!(out.tooltip.contains("&lt;b&gt;"), "{}", out.tooltip);
        assert!(out.tooltip.contains("&amp; co"), "{}", out.tooltip);
        assert!(!out.tooltip.contains('<'), "{}", out.tooltip);
    }

    #[test]
    fn text_reports_the_cord_and_the_battery() {
        let reading = laptop(State::Unplugged, bat("BAT0", Status::Discharging, 85.0, Some(3.99)));
        assert_eq!(
            text(Scope::All, &reading, &Config::default(), false),
            "AC: unplugged\nBAT0: discharging 85% (3h 59m remaining)"
        );
    }

    #[test]
    fn charging_is_captioned_as_time_until_full() {
        let reading = laptop(State::Plugged, bat("BAT0", Status::Charging, 40.0, Some(1.5)));
        assert_eq!(
            text(Scope::All, &reading, &Config::default(), false),
            "AC: plugged in\nBAT0: charging 40% (1h 30m until full)"
        );
    }

    #[test]
    fn a_battery_with_no_runtime_drops_that_part_of_the_line() {
        let reading = laptop(State::Unplugged, bat("BAT0", Status::Discharging, 85.0, None));
        assert_eq!(
            text(Scope::Battery, &reading, &Config::default(), false),
            "BAT0: discharging 85%"
        );
    }

    #[test]
    fn several_batteries_get_a_combined_line_first() {
        let reading = Reading {
            sources: vec![ac_source("AC", State::Unplugged)],
            batteries: vec![
                bat("BAT0", Status::Discharging, 80.0, Some(2.0)),
                bat("BAT1", Status::Discharging, 40.0, Some(2.0)),
            ],
        };
        assert_eq!(
            text(Scope::All, &reading, &Config::default(), false),
            "AC: unplugged\n\
             Battery: discharging 60% (2h 00m remaining)\n\
             \x20 BAT0: discharging 80% (2h 00m remaining)\n\
             \x20 BAT1: discharging 40% (2h 00m remaining)"
        );
    }

    #[test]
    fn scope_narrows_the_report() {
        let reading = laptop(State::Unplugged, bat("BAT0", Status::Discharging, 85.0, Some(3.99)));
        let config = Config::default();
        assert_eq!(text(Scope::Ac, &reading, &config, false), "AC: unplugged");
        assert_eq!(
            text(Scope::Battery, &reading, &config, false),
            "BAT0: discharging 85% (3h 59m remaining)"
        );
    }

    #[test]
    fn the_cord_is_green_plugged_and_yellow_unplugged() {
        let palette = Palette::default();
        assert_eq!(ac_colour(State::Plugged, &palette), Colour::GREEN);
        assert_eq!(ac_colour(State::Unplugged, &palette), Colour::YELLOW);
    }

    #[test]
    fn a_low_battery_speaks_louder_than_its_status() {
        let (levels, palette) = (Levels::default(), Palette::default());
        let shade = |status, percent| battery_colour(status, Some(percent), &levels, &palette);
        // Comfortable: the cord already says whether you are plugged in.
        assert_eq!(shade(Status::Discharging, 85.0), Colour::DEFAULT);
        assert_eq!(shade(Status::Charging, 85.0), Colour::GREEN);
        assert_eq!(shade(Status::Full, 100.0), Colour::GREEN);
        // A deliberate hold at a charge threshold is not a problem.
        assert_eq!(shade(Status::NotCharging, 80.0), Colour::DEFAULT);
        // Low enough to plan around, then low enough to act on.
        assert_eq!(shade(Status::Discharging, 25.0), Colour::YELLOW);
        assert_eq!(shade(Status::Discharging, 10.0), Colour::RED);
        // Recovering, but still not something you can walk away with.
        assert_eq!(shade(Status::Charging, 10.0), Colour::YELLOW);
        assert_eq!(shade(Status::Charging, 25.0), Colour::YELLOW);
        // Nothing legible to report.
        assert_eq!(shade(Status::Unknown, 50.0), Colour::RED);
        assert_eq!(battery_colour(Status::Discharging, None, &levels, &palette), Colour::DEFAULT);
    }

    #[test]
    fn configured_thresholds_move_where_the_colour_changes() {
        let mut config = Config::default();
        config.levels = Levels { full: 98.0, warning: 60.0, critical: 40.0 };
        let shade = |percent| {
            battery_colour(Status::Discharging, Some(percent), &config.levels, &config.palette)
        };
        assert_eq!(shade(70.0), Colour::DEFAULT);
        assert_eq!(shade(55.0), Colour::YELLOW);
        assert_eq!(shade(35.0), Colour::RED);
    }

    #[test]
    fn a_configured_palette_replaces_the_defaults() {
        let mut config = Config::default();
        config.palette.discharging = Colour::CYAN;
        config.palette.unplugged = Colour::MAGENTA;
        let reading = laptop(State::Unplugged, bat("BAT0", Status::Discharging, 85.0, None));
        assert_eq!(
            text(Scope::All, &reading, &config, true),
            "\x1b[35mAC: unplugged\x1b[0m\n\x1b[36mBAT0: discharging 85%\x1b[0m"
        );
    }

    #[test]
    fn configured_formats_replace_the_default_lines() {
        let mut config = Config::default();
        config.formats.battery =
            Template::parse("{name} {level} {percent}%[ / {time}]", crate::config::BATTERY_FIELDS)
                .unwrap();
        let reading = laptop(State::Unplugged, bat("BAT0", Status::Discharging, 12.0, Some(0.5)));
        assert_eq!(
            text(Scope::Battery, &reading, &config, false),
            "BAT0 critical 12% / 30m"
        );
    }

    #[test]
    fn colour_wraps_the_line_only_when_asked() {
        let reading = Reading {
            sources: vec![ac_source("AC", State::Plugged)],
            batteries: vec![],
        };
        let config = Config::default();
        assert_eq!(text(Scope::Ac, &reading, &config, false), "AC: plugged in");
        assert_eq!(text(Scope::Ac, &reading, &config, true), "\x1b[32mAC: plugged in\x1b[0m");
    }

    #[test]
    fn each_line_is_shaded_on_its_own() {
        let reading = laptop(State::Unplugged, bat("BAT0", Status::Discharging, 10.0, None));
        assert_eq!(
            text(Scope::All, &reading, &Config::default(), true),
            "\x1b[33mAC: unplugged\x1b[0m\n\x1b[31mBAT0: discharging 10%\x1b[0m"
        );
    }

    #[test]
    fn the_json_is_never_coloured() {
        let reading = laptop(State::Unplugged, bat("BAT0", Status::Discharging, 10.0, None));
        assert!(!waybar(Scope::All, &reading, &Config::default()).to_json().contains('\x1b'));
    }

    #[test]
    fn ac_scope_reports_one_class() {
        let reading =
            Reading { sources: vec![ac_source("AC", State::Plugged)], batteries: vec![] };
        assert_eq!(
            waybar(Scope::Ac, &reading, &Config::default()).to_json(),
            r#"{"text":"AC","alt":"plugged","class":["plugged"],"tooltip":"AC: plugged in"}"#
        );
    }

    #[test]
    fn battery_scope_reports_status_and_level() {
        let reading = laptop(State::Unplugged, bat("BAT0", Status::Discharging, 12.0, Some(0.5)));
        assert_eq!(
            waybar(Scope::Battery, &reading, &Config::default()).to_json(),
            r#"{"text":"12% 30m","alt":"discharging","class":["discharging","critical"],"tooltip":"BAT0: discharging 12% (30m remaining)","percentage":12}"#
        );
    }

    #[test]
    fn the_combined_module_styles_the_cord_too() {
        let reading = laptop(State::Plugged, bat("BAT0", Status::Charging, 40.0, Some(1.5)));
        let out = waybar(Scope::All, &reading, &Config::default());
        assert_eq!(out.classes, ["charging", "good", "plugged"]);
    }

    #[test]
    fn a_status_and_level_that_agree_are_not_repeated() {
        let reading =
            Reading { sources: vec![], batteries: vec![bat("BAT0", Status::Full, 100.0, None)] };
        assert_eq!(waybar(Scope::Battery, &reading, &Config::default()).classes, ["full"]);
    }

    #[test]
    fn a_machine_without_a_battery_falls_back_to_the_cord() {
        let reading =
            Reading { sources: vec![ac_source("AC", State::Plugged)], batteries: vec![] };
        let out = waybar(Scope::All, &reading, &Config::default());
        assert_eq!(out.alt, "plugged");
        assert_eq!(out.percentage, None);
    }

    #[test]
    fn output_stays_valid_json_for_awkward_names() {
        let reading = Reading {
            sources: vec![ac_source("a\"b\\c", State::Plugged)],
            batteries: vec![],
        };
        assert_eq!(
            waybar(Scope::Ac, &reading, &Config::default()).to_json(),
            r#"{"text":"a\"b\\c","alt":"plugged","class":["plugged"],"tooltip":"a\"b\\c: plugged in"}"#
        );
    }
}
