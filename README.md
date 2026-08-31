# power-module

AC adapter and battery state — as lines of text on the terminal, or as one JSON
object for a waybar custom module. Configurable through an optional TOML file.
One dependency: `toml`.

## Building

```sh
cargo build --release
install -Dm755 target/release/power-module ~/.local/bin/power-module
```

Or with [mise](https://mise.jdx.dev), via the tasks in `.mise.toml`:

```sh
mise run build            # cargo build --release
mise run test             # cargo test; extra args pass through after --
mise run install          # build, then install to ~/.local/bin
PREFIX=/usr/local mise run install
```

## On the command line

```console
$ power-module
AC: unplugged
BAT0: discharging 83% (3h 58m remaining)

$ power-module --battery
BAT0: charging 40% (1h 30m until full)

$ power-module --ac
AC: plugged in

$ power-module --waybar
{"text":"83% 3h 58m","alt":"discharging","class":["discharging","good","unplugged"],"tooltip":"AC: unplugged\nBAT0: discharging 83% (3h 58m remaining)","percentage":83}

$ power-module --quiet && echo "on mains"
```

| Option | |
| --- | --- |
| `--ac` | only the AC adapter |
| `--battery` | only the batteries |
| `-w`, `--waybar` | one JSON object for waybar (`--json` also works) |
| `-f`, `--full` | add the supporting numbers under each line |
| `-q`, `--quiet` | print nothing; exit `0` on external power, `1` on battery |
| `--color <WHEN>` | `auto` (default), `always`, `never` |
| `--config <PATH>` | read this file instead of searching |
| `--no-config` | ignore any config file |
| `-a`, `--adapter <NAME>` | read this supply as the AC adapter |
| `-h`, `--help` / `-V`, `--version` | |

With no scope flag both halves are reported. `--quiet` always answers the one
question "am I on the cord?", whatever the scope.

Exit status is `0` on success, `1` for `--quiet` on battery, and `2` when the
state could not be determined.

### Colour

`auto` colours only when writing to a terminal, so a pipe or a script gets clean
text; `NO_COLOR` in the environment turns it off. The basic eight ANSI colours
are used rather than fixed RGB, so the output follows the terminal's own theme.
JSON is never coloured — style it with CSS instead.

The cord is green plugged in and yellow off it. The battery is coloured by how
much trouble you are in, because that is the one thing the cord cannot tell you:

| | |
| --- | --- |
| charging, full, or plugged in and holding | green |
| discharging | terminal default |
| at or below 30% | yellow |
| at or below 15% | red — or yellow while charging, since it is recovering |
| unreadable | red |

Green means "nothing to do": charging, full, or plugged in and holding at a
level the firmware is happy with. A *discharging* battery above 30% is left
uncoloured on purpose — painting it green too would make green mean nothing at
all, and red stays rare enough to be worth reacting to. To colour that case as
well, set `discharging = "green"` under `[colors]`.

## Configuration

Entirely optional — with no file, everything below is what you get. The file is
looked for as `power-module.toml` in `$XDG_CONFIG_HOME` (default `~/.config`)
and then in each of `$XDG_CONFIG_DIRS` (default `/etc/xdg`), either loose or in
a `power-module/` subdirectory. The first one found wins. `--config` names a
file directly; `--no-config` ignores the lot.

`power-module.toml.example` in this repo is the whole schema with every default
written out, so copying it changes nothing:

```sh
cp power-module.toml.example ~/.config/power-module.toml
```

Command-line flags always beat the file.

### Levels

```toml
[levels]
full = 98
warning = 30
critical = 15
```

These bands decide both the terminal colour and the CSS class waybar receives.
The defaults match the `states` of waybar's own battery module. Thresholds that
would leave a band unreachable — `critical` above `warning`, or `warning` at or
above `full` — are rejected when the file loads rather than silently swallowing
a band.

### Colours

Terminal only; waybar takes its colours from CSS. Named colours rather than RGB,
so output follows the terminal's own theme:

```toml
[colors]
plugged = "green"
unplugged = "yellow"
charging = "green"
full = "green"
discharging = "default"      # the terminal's own foreground
not_charging = "green"       # plugged in and holding: nothing to do
warning = "yellow"
critical = "red"
critical_charging = "yellow"
unknown = "red"
```

`default`, `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`,
`white`, `grey`, and a `bright-` variant of each. `[colours]` also works.

### Formats

```toml
[formats]
ac = "{name}: {state}"
battery = "{name}: {status}[ {percent}%][ ({time} {caption})]"
summary = "Battery: {status}[ {percent}%][ ({time} {caption})]"
bar = "[{percent}%][ {time}]"
bar_ac = "{name}"
```

`{field}` interpolates a value. `[...]` is an **optional group**: if any field
inside it is unknown, the whole group disappears — brackets, literal text and
all. That is what lets one template cover a battery that publishes a runtime and
one that does not:

```text
{name}: {status}[ {percent}%][ ({time} {caption})]
  -> BAT0: discharging 85% (3h 59m remaining)
  -> BAT0: discharging 85%          no runtime published
  -> BAT0: discharging              no level either
```

Write `{{`, `}}`, `[[`, `]]` for literal braces and brackets. `ac` and `bar_ac`
take `{name}` and `{state}`; `battery` takes `{name}`, `{status}`, `{percent}`,
`{time}`, `{caption}` and `{level}`; `summary` and `bar` take the same minus
`{name}`. `ac`, `battery` and `summary` are the terminal lines; `bar` and
`bar_ac` are the waybar `text` field, i.e. what `{}` renders as in a module's
`format`.

Waybar renders module text as **Pango markup**, so a format string may carry
`<span …>` the way waybar's own modules do — nothing this program produces is
escaped on your behalf. The flip side is that a literal `<`, `>` or `&` in a
template will break the label; either escape it yourself (`&lt;`) or set
`"escape": true` on the module. Values the program generates are markup-safe:
durations read `0m` rather than `<1m` for this reason, and error text quoted
back from your config file is escaped before it reaches a tooltip.

### Defaults for the flags

```toml
[general]
scope = "all"        # ac | battery | all
adapter = "AC"       # omit to pick automatically
color = "auto"       # auto | always | never
```

### When the file is wrong

Mistakes are reported when the file loads, not silently ignored — a typo in a
config file you never look at again is worse than a loud failure:

```console
$ power-module
power-module: ~/.config/power-module.toml: levels.critcal: unknown setting; this section takes full, warning, critical
```

Unknown sections and keys, misspelled colours, out-of-order thresholds, unknown
`{placeholders}` and malformed templates are all caught this way. In waybar mode
the module renders as `unknown` with the reason in the tooltip instead, so a bad
config shows up in the bar rather than blanking it.

## In waybar

Two modules, so the cord and the charge can be styled and placed separately:

```jsonc
"custom/ac": {
    "exec": "$HOME/.local/bin/power-module --waybar --ac",
    "return-type": "json",
    "interval": 5,
    "format": "{icon}",
    "format-icons": {
        "plugged": "󰚥",
        "unplugged": "󰚦",
        "unknown": "󰠠"
    },
    "tooltip": true
},

"custom/battery": {
    "exec": "$HOME/.local/bin/power-module --waybar --battery",
    "return-type": "json",
    "interval": 5,
    "format": "{icon} {}",
    "format-icons": {
        "charging":     ["󰢜", "󰂆", "󰂈", "󰂉", "󰂊", "󰂋", "󰂅"],
        "discharging":  ["󰁺", "󰁻", "󰁼", "󰁽", "󰁾", "󰁿", "󰂀", "󰂁", "󰂂", "󰁹"],
        "not-charging": "󰂃",
        "full":         "󰁹",
        "unknown":      "󰂑"
    },
    "tooltip": true
}
```

Or drop `--ac`/`--battery` for a single module covering both: the bar then shows
the battery, and the tooltip carries the cord state as well.

The state is reported four ways, so you can use whichever suits:

- `text` — `83% 3h 58m` for the battery, or the adapter name for `--ac`.
- `alt` — `charging` / `discharging` / `full` / `not-charging` / `unknown`, or
  `plugged` / `unplugged` for `--ac`. This is what `format-icons` keys off; give
  it a list per state, as above, and waybar indexes it by `percentage`.
- `class` — an array: the status, the charge level (`full` / `good` / `warning`
  / `critical`, the same bands as waybar's own battery module), and, in the
  combined module, the cord state. Combine them in CSS the way waybar's battery
  module does: `#custom-battery.discharging.critical`.
- `percentage` — the number on its own, for `{percentage}` in `format`.

```css
/* the cord */
#custom-ac.plugged   { color: #66ff00; }
#custom-ac.unplugged { color: yellow; }
#custom-ac.unknown   { color: #f53c3c; }

/* the battery: the level speaks first, the status fills in the quiet cases */
#custom-battery.charging,
#custom-battery.full,
#custom-battery.not-charging { color: #66ff00; }
#custom-battery.discharging  { color: #ffffff; }
#custom-battery.warning      { color: yellow; }
#custom-battery.unknown      { color: #f53c3c; }

#custom-battery.charging.critical { color: yellow; }

#custom-battery.critical:not(.charging) {
    background-color: #f53c3c;
    color: #ffffff;
    animation-name: blink;
    animation-duration: 0.5s;
    animation-timing-function: steps(12);
    animation-iteration-count: infinite;
    animation-direction: alternate;
}
```

Order matters: `.warning` comes after `.discharging` so it wins at equal
specificity, and `.charging.critical` outranks `.charging` on its own.

With `interval` the bar updates within a few seconds of plugging in. For an
instant update instead, drop the `interval`, set `"signal": 8`, and have a udev
rule fire `pkill -RTMIN+8 waybar` on power supply changes.

## What it reads

Supplies live under `/sys/class/power_supply`, each with a `type` and a handful
of attribute files.

**The adapter.** With no `--adapter`, the module reads every `Mains` supply — on
this laptop that is `AC`. Machines that charge only over USB-C expose no `Mains`
supply at all, so it falls back to the `USB` source ports and reports plugged in
when any one of them is online; the tooltip then breaks out each port.

**The battery.** `status` gives charging / discharging / full / not charging —
the last is what a charge threshold looks like. `capacity` gives the level,
falling back to the contents-over-full ratio for drivers that omit it. Remaining
time is contents divided by rate: either `energy_now` (µWh) over `power_now`
(µW), or `charge_now` (µAh) over `current_now` (µA), whichever pair the driver
publishes.

Machines with more than one battery get a combined line as well as one line per
cell. The overall level is weighted by capacity rather than averaged, so a large
cell counts for more than a small one, and only the cells actually moving charge
count towards the runtime. Energy and charge units are never summed together; if
two batteries disagree on units the combined estimate is dropped rather than
faked.

Nothing is guessed. A battery with no rate published, one sitting idle, or one
that has just been plugged in and still reports a rate of zero simply gets no
time estimate. If a supply reports an unreadable value, that surfaces in waybar
as the `unknown` state with the reason in the tooltip, rather than a silent
claim that you are on battery.

## Tests

```sh
cargo test
```

The readers are tested against throwaway sysfs trees, so the adapter selection,
the two unit systems, and the multi-battery arithmetic are all covered without
needing a second laptop or an unplugged cord. The config parser, the template
language and the colour rules are covered directly.
