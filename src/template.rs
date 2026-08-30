//! The little format language the config file uses for output lines.
//!
//! `{field}` interpolates a value. `[...]` marks an optional group: if any
//! field inside it is unknown, the whole group vanishes, brackets and literal
//! text and all. That is what lets one template cover a battery that reports a
//! runtime and one that does not:
//!
//! ```text
//! {name}: {status}[ {percent}%][ ({time} {caption})]
//!   -> BAT0: discharging 85% (3h 59m remaining)
//!   -> BAT0: discharging 85%              (no runtime published)
//!   -> BAT0: discharging                  (no level either)
//! ```
//!
//! `{{`, `}}`, `[[` and `]]` are the literal characters.

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Literal(String),
    Field(String),
    /// Dropped entirely when one of its fields is unknown.
    Optional(Vec<Node>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    nodes: Vec<Node>,
}

/// The values a template is rendered against; `None` means "not known", which
/// is what makes an optional group disappear.
pub type Fields<'a> = [(&'a str, Option<String>)];

impl Template {
    /// Parses a template and checks every field against `allowed`, so a typo in
    /// the config is caught at load time rather than silently printing nothing.
    pub fn parse(source: &str, allowed: &[&str]) -> Result<Template, String> {
        let (nodes, rest) = Self::parse_nodes(source, allowed, false)?;
        if !rest.is_empty() {
            return Err("unmatched ']'".to_string());
        }
        Ok(Template { nodes })
    }

    fn parse_nodes<'a>(
        input: &'a str,
        allowed: &[&str],
        nested: bool,
    ) -> Result<(Vec<Node>, &'a str), String> {
        let mut nodes = Vec::new();
        let mut literal = String::new();
        let mut rest = input;

        macro_rules! flush {
            () => {
                if !literal.is_empty() {
                    nodes.push(Node::Literal(std::mem::take(&mut literal)));
                }
            };
        }

        while let Some(c) = rest.chars().next() {
            let after = &rest[c.len_utf8()..];
            match c {
                // Doubled brackets and braces are the literal characters.
                '{' | '}' | '[' | ']' if after.starts_with(c) => {
                    literal.push(c);
                    rest = &after[c.len_utf8()..];
                }
                '{' => {
                    let end = after.find('}').ok_or("unmatched '{'")?;
                    let field = after[..end].trim();
                    if field.is_empty() {
                        return Err("empty '{}' placeholder".to_string());
                    }
                    if !allowed.contains(&field) {
                        return Err(format!(
                            "unknown placeholder {{{field}}}; this line takes {}",
                            allowed
                                .iter()
                                .map(|name| format!("{{{name}}}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    flush!();
                    nodes.push(Node::Field(field.to_string()));
                    rest = &after[end + 1..];
                }
                '}' => return Err("unmatched '}'".to_string()),
                '[' => {
                    let (inner, remainder) = Self::parse_nodes(after, allowed, true)?;
                    if inner.iter().any(|node| matches!(node, Node::Optional(_))) {
                        return Err("optional groups cannot be nested".to_string());
                    }
                    flush!();
                    nodes.push(Node::Optional(inner));
                    rest = remainder;
                }
                ']' if nested => {
                    flush!();
                    return Ok((nodes, after));
                }
                ']' => return Err("unmatched ']'".to_string()),
                _ => {
                    literal.push(c);
                    rest = after;
                }
            }
        }

        if nested {
            return Err("unmatched '['".to_string());
        }
        flush!();
        Ok((nodes, rest))
    }

    /// Renders against the given fields. The result is trimmed, so a template
    /// whose leading group drops out does not leave a stray space behind.
    pub fn render(&self, fields: &Fields<'_>) -> String {
        let mut out = String::new();
        Self::render_nodes(&self.nodes, fields, &mut out);
        out.trim().to_string()
    }

    fn render_nodes(nodes: &[Node], fields: &Fields<'_>, out: &mut String) {
        for node in nodes {
            match node {
                Node::Literal(text) => out.push_str(text),
                Node::Field(name) => {
                    if let Some(value) = lookup(fields, name) {
                        out.push_str(value);
                    }
                }
                Node::Optional(inner) => {
                    let complete = inner.iter().all(|node| match node {
                        Node::Field(name) => lookup(fields, name).is_some(),
                        _ => true,
                    });
                    if complete {
                        Self::render_nodes(inner, fields, out);
                    }
                }
            }
        }
    }
}

fn lookup<'a>(fields: &'a Fields<'_>, name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(field, _)| *field == name)
        .and_then(|(_, value)| value.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALLOWED: &[&str] = &["name", "status", "percent", "time", "caption"];

    fn fields(percent: Option<&str>, time: Option<&str>) -> Vec<(&'static str, Option<String>)> {
        vec![
            ("name", Some("BAT0".to_string())),
            ("status", Some("discharging".to_string())),
            ("percent", percent.map(str::to_string)),
            ("time", time.map(str::to_string)),
            ("caption", Some("remaining".to_string())),
        ]
    }

    fn render(source: &str, percent: Option<&str>, time: Option<&str>) -> String {
        Template::parse(source, ALLOWED).unwrap().render(&fields(percent, time))
    }

    #[test]
    fn the_default_battery_line_covers_all_three_cases() {
        let source = "{name}: {status}[ {percent}%][ ({time} {caption})]";
        assert_eq!(
            render(source, Some("85"), Some("3h 59m")),
            "BAT0: discharging 85% (3h 59m remaining)"
        );
        assert_eq!(render(source, Some("85"), None), "BAT0: discharging 85%");
        assert_eq!(render(source, None, None), "BAT0: discharging");
    }

    #[test]
    fn a_leading_group_that_drops_leaves_no_stray_space() {
        assert_eq!(render("[{percent}%][ {time}]", Some("85"), Some("3h")), "85% 3h");
        assert_eq!(render("[{percent}%][ {time}]", None, Some("3h")), "3h");
        assert_eq!(render("[{percent}%][ {time}]", None, None), "");
    }

    #[test]
    fn a_bare_unknown_field_just_vanishes() {
        assert_eq!(render("{status} {percent}", None, None), "discharging");
    }

    #[test]
    fn doubled_brackets_are_literal_characters() {
        assert_eq!(render("{{{percent}}} [[{status}]]", Some("85"), None), "{85} [discharging]");
    }

    #[test]
    fn unknown_placeholders_are_caught_at_parse_time() {
        let error = Template::parse("{name}: {battery_health}", ALLOWED).unwrap_err();
        assert!(error.contains("unknown placeholder {battery_health}"), "{error}");
        assert!(error.contains("{status}"), "{error}");
    }

    #[test]
    fn malformed_templates_are_rejected() {
        for source in ["{name", "name}", "[{name}", "{name}]", "{}", "[[{name}]"] {
            assert!(Template::parse(source, ALLOWED).is_err(), "{source:?} should not parse");
        }
    }

    #[test]
    fn groups_cannot_nest() {
        assert!(Template::parse("[a [b] c]", ALLOWED).is_err());
    }

    #[test]
    fn a_group_with_no_fields_is_always_kept() {
        assert_eq!(render("[literal] {status}", None, None), "literal discharging");
    }
}
