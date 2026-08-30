//! Whether the machine is drawing external power.

use std::path::Path;

use crate::sysfs::{self, Error};

/// Whether the machine is on external power.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Plugged,
    Unplugged,
}

impl State {
    /// Stable machine-readable token, used as the waybar `alt` and CSS class.
    pub fn as_str(self) -> &'static str {
        match self {
            State::Plugged => "plugged",
            State::Unplugged => "unplugged",
        }
    }

    /// Human phrasing for the CLI and the tooltip.
    pub fn describe(self) -> &'static str {
        match self {
            State::Plugged => "plugged in",
            State::Unplugged => "unplugged",
        }
    }
}

/// One external supply the kernel knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub kind: String,
    pub state: State,
}

fn parse_online(path: &Path, raw: String) -> Result<State, Error> {
    match raw.as_str() {
        "1" => Ok(State::Plugged),
        "0" => Ok(State::Unplugged),
        _ => Err(Error::BadValue { path: path.to_path_buf(), raw }),
    }
}

/// Read one supply by directory, reporting every failure.
fn read_source(dir: &Path) -> Result<Source, Error> {
    let online = dir.join("online");
    Ok(Source {
        name: sysfs::name_of(dir),
        kind: sysfs::read_trimmed(&dir.join("type"))?,
        state: parse_online(&online, sysfs::read_trimmed(&online)?)?,
    })
}

/// Read one supply during a scan, skipping anything that is not an external
/// source we understand. Batteries have no `online`, and supplies can vanish
/// mid-scan when a dock is unplugged; neither is an error worth reporting.
fn scan_source(dir: &Path) -> Option<Source> {
    let kind = sysfs::attr(dir, "type")?;
    if kind != "Mains" && kind != "USB" {
        return None;
    }
    let online = dir.join("online");
    let state = parse_online(&online, sysfs::attr(dir, "online")?).ok()?;
    Some(Source { name: sysfs::name_of(dir), kind, state })
}

/// The external supplies to report on, in stable order.
///
/// With no `adapter` given this prefers the mains supplies; only when a machine
/// exposes none (some USB-C-only laptops) does it fall back to the USB source
/// ports, which is why several sources can come back at once.
pub fn sources(root: &Path, adapter: Option<&str>) -> Result<Vec<Source>, Error> {
    if let Some(name) = adapter {
        let dir = root.join(name);
        if !dir.is_dir() {
            return Err(Error::UnknownSupply(name.to_string()));
        }
        return Ok(vec![read_source(&dir)?]);
    }

    let (mains, usb): (Vec<Source>, Vec<Source>) = sysfs::supply_dirs(root)?
        .iter()
        .filter_map(|dir| scan_source(dir))
        .partition(|source| source.kind == "Mains");

    if !mains.is_empty() {
        Ok(mains)
    } else if !usb.is_empty() {
        Ok(usb)
    } else {
        Err(Error::NoAcAdapter)
    }
}

/// The machine is on external power as soon as any one source is online.
pub fn state_of(sources: &[Source]) -> State {
    if sources.iter().any(|source| source.state == State::Plugged) {
        State::Plugged
    } else {
        State::Unplugged
    }
}

/// What to call the set of sources: the supply's own name when there is just
/// one, otherwise something that covers them all.
pub fn label(sources: &[Source]) -> String {
    match sources {
        [only] => only.name.clone(),
        _ => "External power".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FakeRoot;

    fn names(sources: &[Source]) -> Vec<&str> {
        sources.iter().map(|source| source.name.as_str()).collect()
    }

    #[test]
    fn reads_a_plugged_mains_adapter() {
        let root = FakeRoot::new(&[
            ("AC", &[("type", "Mains"), ("online", "1")]),
            ("BAT0", &[("type", "Battery")]),
        ]);
        let sources = sources(root.path(), None).unwrap();
        assert_eq!(names(&sources), ["AC"]);
        assert_eq!(state_of(&sources), State::Plugged);
    }

    #[test]
    fn batteries_are_not_power_sources() {
        let root = FakeRoot::new(&[("BAT0", &[("type", "Battery")])]);
        assert!(matches!(sources(root.path(), None), Err(Error::NoAcAdapter)));
    }

    #[test]
    fn mains_wins_over_usb_ports() {
        let root = FakeRoot::new(&[
            ("AC", &[("type", "Mains"), ("online", "0")]),
            ("ucsi-source-psy-USBC000:001", &[("type", "USB"), ("online", "1")]),
        ]);
        let sources = sources(root.path(), None).unwrap();
        assert_eq!(names(&sources), ["AC"]);
        assert_eq!(state_of(&sources), State::Unplugged);
    }

    #[test]
    fn falls_back_to_usb_ports_and_any_online_counts() {
        let root = FakeRoot::new(&[
            ("ucsi-source-psy-USBC000:001", &[("type", "USB"), ("online", "0")]),
            ("ucsi-source-psy-USBC000:002", &[("type", "USB"), ("online", "1")]),
        ]);
        let sources = sources(root.path(), None).unwrap();
        assert_eq!(
            names(&sources),
            ["ucsi-source-psy-USBC000:001", "ucsi-source-psy-USBC000:002"]
        );
        assert_eq!(state_of(&sources), State::Plugged);
        assert_eq!(label(&sources), "External power");
    }

    #[test]
    fn named_adapter_is_used_verbatim() {
        let root = FakeRoot::new(&[
            ("AC", &[("type", "Mains"), ("online", "1")]),
            ("ucsi-source-psy-USBC000:001", &[("type", "USB"), ("online", "0")]),
        ]);
        let sources = sources(root.path(), Some("ucsi-source-psy-USBC000:001")).unwrap();
        assert_eq!(names(&sources), ["ucsi-source-psy-USBC000:001"]);
        assert_eq!(state_of(&sources), State::Unplugged);
    }

    #[test]
    fn named_adapter_must_exist() {
        let root = FakeRoot::new(&[("AC", &[("type", "Mains"), ("online", "1")])]);
        assert!(matches!(sources(root.path(), Some("nope")), Err(Error::UnknownSupply(_))));
    }

    #[test]
    fn a_garbled_online_flag_is_an_error_not_a_guess() {
        let root = FakeRoot::new(&[("AC", &[("type", "Mains"), ("online", "yes")])]);
        assert!(matches!(sources(root.path(), Some("AC")), Err(Error::BadValue { .. })));
    }
}
