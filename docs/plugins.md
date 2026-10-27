# UI plugins

A UI plugin gives LIVI a different look without a custom build. It is a folder with a `plugin.json` and
[Slint](https://slint.dev) files. It can:

- add **pages** (they appear as tabs, before LIVI's own);
- put **bars** around the pages: `start` (left), `end` (right), `top` and `bottom`. A bar at the side replaces LIVI's
  navigation;
- draw a **background** behind everything;
- show a **start picture** that fades into the UI once it has drawn; this makes for a seamless handover from a
  boot splash;
- show some of LIVI's pages **without its bars** (e.g. a camera picture full screen).

LIVI's own pages (projection, media, dashboard, camera, settings) stay available and keep working. The plugin only
gets data and asks for things through a small, versioned interface, so it needs no changes in LIVI.

The Slint interpreter compiles the plugin when LIVI starts (a few hundred milliseconds on a small ARM board). If the
plugin has errors, LIVI logs them and starts without it.

## Where LIVI looks

- `~/.config/LIVI/ui-plugin/` (macOS: `~/Library/Application Support/LIVI/ui-plugin/`), when it has a `plugin.json`;
- or the folder in the environment variable `LIVI_UI_PLUGIN`.

## plugin.json

```json
{
  "api": 1,
  "name": "Example",
  "file": "example.slint",
  "start": "hello",
  "splash": "splash.png",
  "shell": { "background": "Background", "start": "Rail", "bottom": "Bar" },
  "pages": [
    { "id": "hello", "title": "Hello", "icon": "icons/hello.svg", "component": "Hello" }
  ],
  "fullscreen": ["/camera"],
  "hide": ["/telemetry"]
}
```

| Key | Meaning |
|---|---|
| `api` | Plugin API the plugin is written for; LIVI refuses others. Currently `1`. |
| `name` | Shown in logs. |
| `file` | Main `.slint` file; it exports every component the manifest names. It may import other files of the folder. |
| `start` | Page shown at start (an `id` from `pages`), instead of LIVI's start page. |
| `splash` | Picture shown full window at start, until the UI has drawn; then it fades out. |
| `shell` | Components for the places around the pages: `background`, `start`, `end`, `top`, `bottom`. Each takes the size it prefers (`preferred-width` at the sides, `preferred-height` at top and bottom). `overlay` covers the whole window above everything, also on full screen pages and over the projection, e.g. for warnings; touches go through it except where it has a `TouchArea`. |
| `empty` | Components for LIVI's pages while they have nothing to show: `projection` (no phone streaming), `camera` (no camera found). |
| `pages` | Pages of the plugin: `id`, `title`, `icon` (file in the folder), `component`; `"tab": false` keeps a page out of the tabs and the order for swiping (e.g. one the plugin opens from its own button); `"settings"` makes it the page for LIVI's settings (see Settings). |
| `fullscreen` | Paths of pages that show without the bars, e.g. `"/camera"`. |
| `hide` | Paths of LIVI's pages the plugin replaces with its own, e.g. `"/media"`: their tabs go away, the paths still work. |

While the phone's projection is streaming, LIVI hides the bars (as it hides its own navigation).

## Components

Every component LIVI shows inherits `Window` (the interpreter only builds exported windows; LIVI embeds them). They
read LIVI's state from the global `Livi` and tell LIVI what the user wants with `Livi.action(verb, argument)`. The main
file exports the global, so that LIVI can reach it (LIVI refuses a plugin that does not):

```slint
import { Livi } from "@livi/api.slint";
export { Livi }

export component Hello inherits Window {
    background: #101418;
    Text { text: Livi.clock; color: white; }
    TouchArea { clicked => { Livi.action("open", "/media"); } }
}
```

### State

`@livi/api.slint` ([native/crates/livi-ui/plugin/api.slint](../native/crates/livi-ui/plugin/api.slint)) defines the
global `Livi` with these properties:

- `page`: the page on screen, a path;
- `clock` and `date`;
- `dark` (dark mode) and `phone` (`none`, `connected`, `streaming`);
- `pages`: the pages in tab order, with title, icon and whether each one is active;
- `media`: title, artist, album, artwork, playing, duration;
- `playback`: where the song is (progress, elapsed time);
- `nav`: turn-by-turn from the phone (maneuver, its icon, distance, road, time left, destination);
- `drive`: speed, rpm and gear;
- `car`: whether the car moves, outside temperature, fuel, range, odometer, lights and so on. Each value of `drive`
  and `car` comes with a `has-*` flag that says whether it arrived;
- `climate`: as the car reports it. `known` is only true for values from outside (a telemetry source). LIVI's
  defaults and the values saved from the last run do not count;
- `other`: telemetry keys LIVI has no field for, as text. A CAN reader can send e.g. `doors` to the telemetry input
  and a plugin shows it; `Livi.value("doors")` gives one of them by its key (`""` when there is none);
- `notices`: notifications other programs sent, newest first. LIVI serves `org.freedesktop.Notifications` on the
  session bus, so any program notifies with the usual tools, e.g. `notify-send -u critical "Title" "Text"`. Each has
  program, summary, body, urgency (`low`, `normal`, `critical`) and the local time it came;
  `Livi.action("notice", "dismiss:<id>")` closes one;
- `can`: the raw values a CAN reader keeps under the telemetry key `can`, as text, apart from `other` because they may
  change many times a second.

LIVI sets each property only when it changed. A binding runs again only when a property it reads changed: one that
shows the speed does not run when the song changes. A struct changes as a whole, though: a binding that reads one
field of `car` runs again when any field of `car` changes. That is why speed, rpm and gear (`drive`) and the song's
position (`playback`) have structs of their own; while driving they change several times a second. So a page causes
no work while nothing it reads changes, as long as nothing on it runs by itself (a timer or an animation).

### Actions

| Call | Effect |
|---|---|
| `Livi.action("open", "/plugin/<id>")` | Shows a page of the plugin. |
| `Livi.action("open", "/media")` and the like | Shows one of LIVI's pages: `/projection`, `/media`, `/telemetry`, `/camera`, `/settings`. |
| `Livi.action("open", "/projection/<app>")` | Brings the projection up with one of the phone's apps, e.g. `maps`, `music`, `mobilephone` (CarPlay). |
| `Livi.action("media", "play-pause")` | Controls the phone's player; also `next` and `prev`. |
| `Livi.action("page", "next")` | The plugin's next page in tab order (`prev`: the previous one), e.g. after a swipe; nothing at the first or last page. |

### Settings

A page with `"settings": ["list", "select", ...]` in the manifest shows LIVI's settings in the plugin's design. The
list names the kinds of screens the page draws: `list`, `select`, `number`, `text`, `color`, `slider`,
`calibration`, `pending`, and `dialog` and `capture` for a dialog over a screen and the wait for a key. LIVI keeps the logic, saving and translations; the page gets the screen on top of LIVI's
settings stack and sends the user's wishes back:

- `settings`: kind, title, whether there is a way back or an apply button, and the values of a number or slider screen;
- `setting-rows`: the rows of a list (kind, label, icon, value, switch, level, enabled, node);
- `setting-picks`: the choices of a select screen;
- `setting-knobs`: the sliders of a calibration screen;
- `setting-dialog-rows`: the rows of a dialog;
- `Livi.action("settings", ...)`: `open:<node>`, `toggle:<row>`, `press:<row>`, `capture:<row>`,
  `slide:<node>:<level>`, `slide-done:<node>:<level>`, `pick:<index>`, `number:<value>`, `level:<level>`,
  `level-done:<level>`, `text:<text>`, `hex:<rrggbb>`, `color-slide:<i>:<level>`, `color-slide-done:<i>:<level>`,
  `color-reset`, `knob:<i>:<level>`, `knob-done:<i>:<level>`, `knob-reset:<i>`, `dialog-press:<row>`,
  `capture-cancel`, `back`, `apply` (`plugin/api.slint` describes each field).

A screen of another kind, a dialog and the wait for a key LIVI shows on its own settings page; once the screen is one
the page draws again, LIVI shows the plugin's page. `/settings` (e.g. from `Livi.action("open", "/settings")`) opens
the plugin's page; `"hide": ["/settings"]` takes LIVI's tab away. The example plugin has a settings page that draws
only lists.

## Tools

- `LIVI --check-plugin <folder>` compiles a plugin without a display and reports its errors.
- Preview without a car or phone: `LIVI_UI_STATE=<state.json> LIVI_UI_PLUGIN=<folder> LIVI` runs the UI from a
  recorded state. The files in `native/crates/livi-ui/fixtures/` are examples. Add `LIVI_UI_SIZE=2400x900` for the
  size of your screen and `LIVI_UI_PAGE=plugin/<id>` to open a page.
- `LIVI_UI_TRACE=1` prints how long the steps of the start take, up to the first frames.

## Example

[native/crates/livi-ui/plugin/example](../native/crates/livi-ui/plugin/example) has one page, a bar with a button
per page and a settings page that draws LIVI's settings lists. The tests compile it.
