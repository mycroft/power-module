//! Battery charge, state, and the time left to empty or full.
//!
//! Drivers report a battery's contents one of two ways: as energy (`energy_now`
//! in µWh, drained at `power_now` µW) or as charge (`charge_now` in µAh, drained
//! at `current_now` µA). Both give the same answer for "how long left" — value
//! divided by rate — but the two units must never be mixed, so which one a
//! battery uses is carried along with its numbers.

use std::path::Path;
use std::time::Duration;

use crate::sysfs::{self, Error};

/// What the battery is doing, as reported by `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Charging,
    Discharging,
    Full,
    /// Plugged in but deliberately not charging — usually a charge threshold.
    NotCharging,
    Unknown,
}

impl Status {
    fn parse(raw: &str) -> Self {
        match raw {
            "Charging" => Status::Charging,
            "Discharging" => Status::Discharging,
            "Full" => Status::Full,
            // "Idle" is what some vendor drivers report at a charge threshold.
            "Not charging" | "Idle" => Status::NotCharging,
            _ => Status::Unknown,
        }
    }

    /// Stable machine-readable token, used as the waybar `alt` and CSS class.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Charging => "charging",
            Status::Discharging => "discharging",
            Status::Full => "full",
            Status::NotCharging => "not-charging",
            Status::Unknown => "unknown",
        }
    }

    /// Human phrasing for the CLI and the tooltip.
    pub fn describe(self) -> &'static str {
        match self {
            Status::Charging => "charging",
            Status::Discharging => "discharging",
            Status::Full => "full",
            Status::NotCharging => "not charging",
            Status::Unknown => "unknown",
        }
    }

    /// How to caption a remaining time for this status.
    pub fn time_caption(self) -> &'static str {
        match self {
            Status::Charging => "until full",
            _ => "remaining",
        }
    }
}

/// How much charge is left, in the bands that decide how loudly to say it.
///
/// These match the `states` of waybar's own battery module, so the same CSS
/// selectors work for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// At or above 98%.
    Full,
    /// The ordinary middle of the range.
    Good,
    /// At or below 30% — worth noticing.
    Warning,
    /// At or below 15% — worth acting on.
    Critical,
}

/// Where the bands sit. Configurable, but these defaults are the `states` of
/// waybar's own battery module.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    pub full: f64,
    pub warning: f64,
    pub critical: f64,
}

impl Default for Levels {
    fn default() -> Self {
        Levels { full: 98.0, warning: 30.0, critical: 15.0 }
    }
}

impl Levels {
    /// Which band a percentage falls in. Checked from the bottom up, so a
    /// battery that is somehow both critical and full reads as critical.
    pub fn of(&self, percent: f64) -> Level {
        if percent <= self.critical {
            Level::Critical
        } else if percent <= self.warning {
            Level::Warning
        } else if percent >= self.full {
            Level::Full
        } else {
            Level::Good
        }
    }
}

impl Level {
    /// Stable machine-readable token, used as a CSS class.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Full => "full",
            Level::Good => "good",
            Level::Warning => "warning",
            Level::Critical => "critical",
        }
    }
}

/// Which pair of units a battery reports its contents in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// µWh drained at µW.
    Energy,
    /// µAh drained at µA.
    Charge,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Battery {
    pub name: String,
    pub status: Status,
    /// Charge level, 0–100.
    pub percent: Option<f64>,
    pub unit: Option<Unit>,
    /// Current contents, in `unit`.
    pub now: Option<f64>,
    /// Contents when full, in `unit`.
    pub full: Option<f64>,
    /// Charge or discharge rate, always a magnitude.
    pub rate: Option<f64>,
}

/// How long until this battery is empty (discharging) or full (charging).
///
/// `None` whenever the answer would be a guess: an idle battery, a rate of
/// zero right after plugging in, or a driver that publishes no rate at all.
pub fn remaining(battery: &Battery) -> Option<Duration> {
    let rate = battery.rate.filter(|rate| *rate > 0.0)?;
    let outstanding = match battery.status {
        Status::Discharging => battery.now?,
        Status::Charging => (battery.full? - battery.now?).max(0.0),
        _ => return None,
    };
    hours_to_duration(outstanding / rate)
}

/// Guards the float arithmetic: `Duration::from_secs_f64` panics on a negative
/// or non-finite input, and a battery reporting a near-zero rate can produce an
/// absurd span rather than a useful one.
fn hours_to_duration(hours: f64) -> Option<Duration> {
    if !hours.is_finite() || hours <= 0.0 || hours > 1000.0 {
        return None;
    }
    Some(Duration::from_secs_f64(hours * 3600.0))
}

fn read_battery(dir: &Path) -> Option<Battery> {
    // A vacant battery bay stays in sysfs with present=0 and stale numbers.
    if sysfs::attr_i64(dir, "present") == Some(0) {
        return None;
    }

    let status = Status::parse(&sysfs::attr(dir, "status").unwrap_or_default());
    let magnitude = |attr: &str| sysfs::attr_i64(dir, attr).map(|value| value.abs() as f64);

    let (unit, now, full, rate) = if let Some(now) = magnitude("energy_now") {
        (Some(Unit::Energy), Some(now), magnitude("energy_full"), magnitude("power_now"))
    } else if let Some(now) = magnitude("charge_now") {
        (Some(Unit::Charge), Some(now), magnitude("charge_full"), magnitude("current_now"))
    } else {
        (None, None, None, None)
    };

    // `capacity` is what the kernel itself reports; the ratio is the fallback
    // for the drivers that leave it out.
    let percent = sysfs::attr_i64(dir, "capacity")
        .map(|value| value as f64)
        .or_else(|| match (now, full) {
            (Some(now), Some(full)) if full > 0.0 => Some(now / full * 100.0),
            _ => None,
        })
        .map(|percent| percent.clamp(0.0, 100.0));

    Some(Battery { name: sysfs::name_of(dir), status, percent, unit, now, full, rate })
}

/// Every battery present in the machine, in stable order.
///
/// An empty list is a normal answer — a desktop has no battery — so this only
/// fails if sysfs itself cannot be read.
pub fn batteries(root: &Path) -> Result<Vec<Battery>, Error> {
    Ok(sysfs::supply_dirs(root)?
        .iter()
        .filter(|dir| sysfs::attr(dir, "type").as_deref() == Some("Battery"))
        .filter_map(|dir| read_battery(dir))
        .collect())
}

/// The whole battery pack treated as one, for machines with more than one cell.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub status: Status,
    pub percent: Option<f64>,
    pub remaining: Option<Duration>,
}

/// Combine several batteries into a single reading.
///
/// Discharging wins over charging: on a machine where one cell tops up while
/// another drains, "you are on battery" is the answer that matters.
pub fn summarise(batteries: &[Battery]) -> Option<Summary> {
    if batteries.is_empty() {
        return None;
    }
    let any = |wanted: Status| batteries.iter().any(|battery| battery.status == wanted);
    let status = if any(Status::Discharging) {
        Status::Discharging
    } else if any(Status::Charging) {
        Status::Charging
    } else if batteries.iter().all(|battery| battery.status == Status::Full) {
        Status::Full
    } else if any(Status::NotCharging) {
        Status::NotCharging
    } else {
        Status::Unknown
    };

    Some(Summary { status, percent: total_percent(batteries), remaining: total_remaining(batteries, status) })
}

/// A big battery and a small one at the same percentage are not worth the same,
/// so weight by capacity where the numbers allow and only fall back to a plain
/// mean when they do not.
fn total_percent(batteries: &[Battery]) -> Option<f64> {
    if let Some((now, full)) = comparable_totals(batteries, |_| true) {
        if full > 0.0 {
            return Some((now / full * 100.0).clamp(0.0, 100.0));
        }
    }
    let known: Vec<f64> = batteries.iter().filter_map(|battery| battery.percent).collect();
    if known.is_empty() {
        return None;
    }
    Some(known.iter().sum::<f64>() / known.len() as f64)
}

/// Sum `now` and `full` across the selected batteries, but only if they all
/// report in the same unit — µWh and µAh cannot be added together.
fn comparable_totals(
    batteries: &[Battery],
    select: impl Fn(&Battery) -> bool,
) -> Option<(f64, f64)> {
    let selected: Vec<&Battery> = batteries.iter().filter(|battery| select(battery)).collect();
    let unit = selected.first()?.unit?;
    if selected.iter().any(|battery| battery.unit != Some(unit)) {
        return None;
    }
    let mut totals = (0.0, 0.0);
    for battery in selected {
        totals.0 += battery.now?;
        totals.1 += battery.full?;
    }
    Some(totals)
}

/// Only the cells actually moving charge count towards the estimate: a full
/// battery sitting alongside a draining one contributes no runtime.
fn total_remaining(batteries: &[Battery], status: Status) -> Option<Duration> {
    let active = |battery: &Battery| battery.status == status;
    let (now, full) = comparable_totals(batteries, active)?;
    let rate: f64 = batteries
        .iter()
        .filter(|battery| active(battery))
        .map(|battery| battery.rate.unwrap_or(0.0))
        .sum();
    if rate <= 0.0 {
        return None;
    }
    let outstanding = match status {
        Status::Discharging => now,
        Status::Charging => (full - now).max(0.0),
        _ => return None,
    };
    hours_to_duration(outstanding / rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FakeRoot;

    fn minutes(duration: Option<Duration>) -> Option<u64> {
        duration.map(|duration| (duration.as_secs() + 30) / 60)
    }

    #[test]
    fn reads_an_energy_reporting_battery() {
        // The real BAT0 of this laptop: 33.65 Wh left, drawing 8.434 W.
        let root = FakeRoot::new(&[(
            "BAT0",
            &[
                ("type", "Battery"),
                ("status", "Discharging"),
                ("present", "1"),
                ("capacity", "85"),
                ("energy_now", "33650000"),
                ("energy_full", "39370000"),
                ("power_now", "8434000"),
            ],
        )]);
        let batteries = batteries(root.path()).unwrap();
        assert_eq!(batteries.len(), 1);
        assert_eq!(batteries[0].status, Status::Discharging);
        assert_eq!(batteries[0].percent, Some(85.0));
        assert_eq!(minutes(remaining(&batteries[0])), Some(239)); // 3h 59m
    }

    #[test]
    fn reads_a_charge_reporting_battery_and_its_time_to_full() {
        let root = FakeRoot::new(&[(
            "BAT0",
            &[
                ("type", "Battery"),
                ("status", "Charging"),
                ("capacity", "50"),
                ("charge_now", "2000000"),
                ("charge_full", "4000000"),
                ("current_now", "2000000"),
            ],
        )]);
        let batteries = batteries(root.path()).unwrap();
        assert_eq!(batteries[0].unit, Some(Unit::Charge));
        assert_eq!(minutes(remaining(&batteries[0])), Some(60));
    }

    #[test]
    fn a_signed_discharge_current_is_a_magnitude() {
        let root = FakeRoot::new(&[(
            "BAT0",
            &[
                ("type", "Battery"),
                ("status", "Discharging"),
                ("charge_now", "2000000"),
                ("charge_full", "4000000"),
                ("current_now", "-1000000"),
            ],
        )]);
        let batteries = batteries(root.path()).unwrap();
        assert_eq!(minutes(remaining(&batteries[0])), Some(120));
    }

    #[test]
    fn capacity_falls_back_to_the_energy_ratio() {
        let root = FakeRoot::new(&[(
            "BAT0",
            &[
                ("type", "Battery"),
                ("status", "Discharging"),
                ("energy_now", "1000000"),
                ("energy_full", "4000000"),
            ],
        )]);
        let batteries = batteries(root.path()).unwrap();
        assert_eq!(batteries[0].percent, Some(25.0));
        // No rate published, so no invented estimate.
        assert_eq!(remaining(&batteries[0]), None);
    }

    #[test]
    fn an_idle_or_zero_rate_battery_gets_no_estimate() {
        let root = FakeRoot::new(&[
            ("BAT0", &[
                ("type", "Battery"),
                ("status", "Full"),
                ("capacity", "100"),
                ("energy_now", "4000000"),
                ("energy_full", "4000000"),
                ("power_now", "0"),
            ]),
        ]);
        let batteries = batteries(root.path()).unwrap();
        assert_eq!(batteries[0].status, Status::Full);
        assert_eq!(remaining(&batteries[0]), None);
    }

    #[test]
    fn a_vacant_bay_is_not_a_battery() {
        let root = FakeRoot::new(&[
            ("BAT0", &[("type", "Battery"), ("status", "Discharging"), ("present", "1"), ("capacity", "50")]),
            ("BAT1", &[("type", "Battery"), ("status", "Unknown"), ("present", "0"), ("capacity", "0")]),
        ]);
        let batteries = batteries(root.path()).unwrap();
        assert_eq!(batteries.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["BAT0"]);
    }

    #[test]
    fn no_battery_is_an_empty_list_not_an_error() {
        let root = FakeRoot::new(&[("AC", &[("type", "Mains"), ("online", "1")])]);
        assert_eq!(batteries(root.path()).unwrap(), vec![]);
        assert_eq!(summarise(&[]), None);
    }

    #[test]
    fn two_batteries_are_weighted_by_size_not_averaged() {
        // 30 Wh of 40 Wh, plus 5 Wh of 10 Wh: 70% overall, not the 62.5% a
        // plain mean of 75% and 50% would give.
        let root = FakeRoot::new(&[
            ("BAT0", &[
                ("type", "Battery"), ("status", "Discharging"), ("capacity", "75"),
                ("energy_now", "30000000"), ("energy_full", "40000000"), ("power_now", "10000000"),
            ]),
            ("BAT1", &[
                ("type", "Battery"), ("status", "Discharging"), ("capacity", "50"),
                ("energy_now", "5000000"), ("energy_full", "10000000"), ("power_now", "5000000"),
            ]),
        ]);
        let summary = summarise(&batteries(root.path()).unwrap()).unwrap();
        assert_eq!(summary.status, Status::Discharging);
        assert_eq!(summary.percent, Some(70.0));
        // 35 Wh left drawn at a combined 15 W.
        assert_eq!(minutes(summary.remaining), Some(140));
    }

    #[test]
    fn a_full_cell_does_not_pad_the_runtime_of_a_draining_one() {
        let root = FakeRoot::new(&[
            ("BAT0", &[
                ("type", "Battery"), ("status", "Full"),
                ("energy_now", "10000000"), ("energy_full", "10000000"), ("power_now", "0"),
            ]),
            ("BAT1", &[
                ("type", "Battery"), ("status", "Discharging"),
                ("energy_now", "10000000"), ("energy_full", "10000000"), ("power_now", "10000000"),
            ]),
        ]);
        let summary = summarise(&batteries(root.path()).unwrap()).unwrap();
        assert_eq!(summary.status, Status::Discharging);
        assert_eq!(minutes(summary.remaining), Some(60));
    }

    #[test]
    fn mixed_units_are_never_added_together() {
        let root = FakeRoot::new(&[
            ("BAT0", &[
                ("type", "Battery"), ("status", "Discharging"), ("capacity", "80"),
                ("energy_now", "8000000"), ("energy_full", "10000000"), ("power_now", "10000000"),
            ]),
            ("BAT1", &[
                ("type", "Battery"), ("status", "Discharging"), ("capacity", "40"),
                ("charge_now", "4000000"), ("charge_full", "10000000"), ("current_now", "1000000"),
            ]),
        ]);
        let summary = summarise(&batteries(root.path()).unwrap()).unwrap();
        // Falls back to the mean of the reported percentages, and declines to
        // invent a combined runtime.
        assert_eq!(summary.percent, Some(60.0));
        assert_eq!(summary.remaining, None);
    }

    #[test]
    fn charge_levels_band_the_percentage() {
        let levels = Levels::default();
        assert_eq!(levels.of(100.0), Level::Full);
        assert_eq!(levels.of(98.0), Level::Full);
        assert_eq!(levels.of(97.9), Level::Good);
        assert_eq!(levels.of(31.0), Level::Good);
        assert_eq!(levels.of(30.0), Level::Warning);
        assert_eq!(levels.of(15.1), Level::Warning);
        assert_eq!(levels.of(15.0), Level::Critical);
        assert_eq!(levels.of(0.0), Level::Critical);
    }

    #[test]
    fn configured_bands_move_the_boundaries() {
        let levels = Levels { full: 90.0, warning: 50.0, critical: 20.0 };
        assert_eq!(levels.of(95.0), Level::Full);
        assert_eq!(levels.of(60.0), Level::Good);
        assert_eq!(levels.of(50.0), Level::Warning);
        assert_eq!(levels.of(20.0), Level::Critical);
    }

    #[test]
    fn a_charge_threshold_reads_as_not_charging() {
        let root = FakeRoot::new(&[(
            "BAT0",
            &[("type", "Battery"), ("status", "Not charging"), ("capacity", "80")],
        )]);
        let summary = summarise(&batteries(root.path()).unwrap()).unwrap();
        assert_eq!(summary.status, Status::NotCharging);
        assert_eq!(summary.percent, Some(80.0));
        assert_eq!(summary.remaining, None);
    }
}
