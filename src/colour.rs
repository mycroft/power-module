//! Terminal colours, as the basic ANSI set.
//!
//! Colours are held as SGR codes rather than RGB so output follows whatever
//! palette the terminal is themed with — a config that says `red` means the
//! user's red.

/// `None` is the terminal's own foreground, i.e. "say nothing".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour(Option<u8>);

impl Colour {
    pub const DEFAULT: Colour = Colour(None);
    pub const BLACK: Colour = Colour(Some(30));
    pub const RED: Colour = Colour(Some(31));
    pub const GREEN: Colour = Colour(Some(32));
    pub const YELLOW: Colour = Colour(Some(33));
    pub const BLUE: Colour = Colour(Some(34));
    pub const MAGENTA: Colour = Colour(Some(35));
    pub const CYAN: Colour = Colour(Some(36));
    pub const WHITE: Colour = Colour(Some(37));

    /// Names accepted in the config file. `bright-` and `bright_` both work, so
    /// a TOML key and a TOML string can be spelled the same way.
    pub fn parse(name: &str) -> Option<Colour> {
        let name = name.trim().to_ascii_lowercase();
        let name = name.replace('_', "-");
        let (bright, base) = match name.strip_prefix("bright-") {
            Some(base) => (true, base),
            None => (false, name.as_str()),
        };
        let colour = match base {
            "default" | "none" => return if bright { None } else { Some(Colour::DEFAULT) },
            "black" => Colour::BLACK,
            "red" => Colour::RED,
            "green" => Colour::GREEN,
            "yellow" => Colour::YELLOW,
            "blue" => Colour::BLUE,
            "magenta" | "purple" => Colour::MAGENTA,
            "cyan" => Colour::CYAN,
            "white" => Colour::WHITE,
            // The usual name for bright black.
            "grey" | "gray" => return Some(Colour(Some(90))),
            _ => return None,
        };
        Some(match (bright, colour.0) {
            (true, Some(code)) => Colour(Some(code + 60)),
            _ => colour,
        })
    }

    /// Every name `parse` accepts, for error messages.
    pub const NAMES: &'static str = "default, black, red, green, yellow, blue, magenta, cyan, \
                                     white, grey, and bright-* variants";

    pub fn paint(self, text: &str, enabled: bool) -> String {
        match (enabled, self.0) {
            (true, Some(code)) => format!("\x1b[{code}m{text}\x1b[0m"),
            _ => text.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_to_codes() {
        assert_eq!(Colour::parse("green"), Some(Colour::GREEN));
        assert_eq!(Colour::parse("  YELLOW "), Some(Colour::YELLOW));
        assert_eq!(Colour::parse("default"), Some(Colour::DEFAULT));
        assert_eq!(Colour::parse("none"), Some(Colour::DEFAULT));
        assert_eq!(Colour::parse("purple"), Some(Colour::MAGENTA));
    }

    #[test]
    fn bright_variants_take_either_separator() {
        assert_eq!(Colour::parse("bright-red"), Colour::parse("bright_red"));
        assert_eq!(Colour::parse("bright-red"), Some(Colour(Some(91))));
        assert_eq!(Colour::parse("grey"), Some(Colour(Some(90))));
        // "bright-default" is not a colour.
        assert_eq!(Colour::parse("bright-default"), None);
    }

    #[test]
    fn unknown_names_are_rejected_rather_than_guessed() {
        assert_eq!(Colour::parse("mauve"), None);
        assert_eq!(Colour::parse("#ff0000"), None);
        assert_eq!(Colour::parse(""), None);
    }

    #[test]
    fn painting_is_opt_in_and_default_never_emits_codes() {
        assert_eq!(Colour::GREEN.paint("x", true), "\x1b[32mx\x1b[0m");
        assert_eq!(Colour::GREEN.paint("x", false), "x");
        assert_eq!(Colour::DEFAULT.paint("x", true), "x");
    }
}
