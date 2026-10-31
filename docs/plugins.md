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
| `name` | Shown in logs and the debug overlay. |
| `file` | Main `.slint` file; it exports every component the manifest names. It may import other files of the folder. |
| `start` | Page shown at start (an `id` from `pages`), instead of LIVI's start page. |
| `splash` | Picture shown full window at start, until the UI has drawn; then it fades out. |
| `shell` | Components for the places around the pages: `background`, `start`, `end`, `top`, `bottom`. Each takes the size it prefers (`preferred-width` at the sides, `preferred-height` at top and bottom). The `background` steps aside while a page shows a video (the phone streaming, a camera picture): the compositor draws the video below LIVI's window, where the window is transparent. The bars stay on the camera page; on the projection they go with LIVI's navigation. With a card set (below) the picture shows as a card instead and the background draws around it. `overlay` covers the whole window above everything, also on full screen pages and over the projection, e.g. for warnings; touches go through it except where it has a `TouchArea`. |
| `empty` | Components for LIVI's pages while they have nothing to show: `projection` (no phone streaming), `camera` (no camera found). |
| `frame` | Components that lay out a page of LIVI around its picture while it shows: `camera`. The component says where the picture goes (`camera-area`, Actions); see Pictures as a card. |
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
- `links`: Wi-Fi and Bluetooth, each `off` (no adapter, or it is switched off), `on` (ready, no phone over it) or
  `connected` (a phone is connected over it). Bluetooth follows BlueZ on the system bus (`off` without it); Wi-Fi is
  `on` while the system has a Wi-Fi interface and `connected` while a phone is present over Wi-Fi;
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
- `video`: the card the phone's or the camera's picture shows in (below), `shown` while it does, with its distances
  from the window's edges and its corner radius in the window's pixels;
- `notices`: notifications other programs sent, newest first. LIVI serves `org.freedesktop.Notifications` on the
  session bus, so any program notifies with the usual tools, e.g. `notify-send -u critical "Title" "Text"`. Each has
  program, summary, body, urgency (`low`, `normal`, `critical`) and the local time it came, and a `serial` that grows
  with every notice that comes or says something new (for banners: what is new since the page last looked);
  `Livi.action("notice", "dismiss:<id>")` closes one;
- `can`: the raw values a CAN reader keeps under the telemetry key `can`, as text, apart from `other` because they may
  change many times a second;
- `extensions`: what programs beside LIVI publish for the plugin (companion channel, below);
  `Livi.extension(topic, key)` gives one value as text;
- `focus`, `press` and `key`: where the focus of the keys and a rotary knob is, which target was activated with a
  key, and the last key LIVI has no use for (Keys, below).

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
| `Livi.action("camera-area", "<x> <y> <width> <height> [<radius> [<px> <py> <pwidth> <pheight>]]")` | From a `frame` component of the camera page: the card the camera's picture shows in (window pixels, e.g. from `absolute-position`), and where the whole picture goes, fitted and centred (`p…`, the card when left out; larger than the card for a picture with borders, also beyond the window). `""` puts it back on the whole screen. |
| `Livi.action("key", "<text>")` | A key of a keyboard the plugin draws, typed into the text field with the focus: a letter or other text, or `backspace`, `return`. Slint's `TextInputInterface.text-input-focused` says when to show the keyboard. |
| `Livi.action("focus-targets", "<scope> <id>[:slider][:current] ...")` | What the keys and a rotary knob move the focus over, for a page or the side bar (Keys, below). |

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

## Keys

Besides touch, LIVI can be driven with keys: a keyboard, a steering wheel or a rotary knob, through the key bindings
(Settings, Key Bindings): the arrows, `selectDown` (Enter), `back`, and the knob's `knobLeft`, `knobRight` (turning)
and `knobDown` (pressing). LIVI keeps the focus in two zones, the navigation and the page on screen, and moves it
itself; the plugin says what its components can take the focus on, draws it, and does what a target stands for.

- **Targets:** `Livi.action("focus-targets", "<scope> <id> <id> ...")` declares the targets of a page in their order,
  by the page's path as scope (`/plugin/<id>`, also `/camera` for a camera frame), or of the plugin's side bar with
  the scope `nav`. An id is any text without spaces and colons. Flags after the id: `:slider` for a target that left
  and right change instead of leaving it, `:current` in the side bar for the button of the page on screen (the focus
  lands there coming back from the page). A new declaration replaces the scope's list, one without ids takes it away;
  at most 256 per list. All pages of the plugin stay alive, so each declares its list once it is created and again
  when it changes. Not from `init` alone: `init` runs while LIVI creates the component, before it connects
  `Livi.action`, so an action there goes nowhere. A `changed` handler on a property LIVI sets runs after that, e.g.
  `property <string> page: Livi.page; changed page => { ... }` (LIVI sets the page right after it created the
  component, and on every change of the page). A component LIVI shows only at times (a camera frame, an empty page)
  declares an empty list when it has nothing to focus, so that no list of another one stays.
- **Navigation:** with a side bar (`shell` `start` or `end`) that declared targets, the navigation is that bar;
  otherwise it is LIVI's tabs, also when a plugin bar lists them (`Livi.pages`).
- **`Livi.focus`:** `zone` is `""` (nothing focused: after a touch, or before the first key), `"nav"` or `"page"`;
  `target` the id there (in the navigation without a declared bar the path of the tab, as in `pages`); `editing` while
  a slider or a number takes the knob. The page draws its focus where `Livi.focus.zone == "page"` and the target is
  one of its ids; a touch takes the focus away.
- **`Livi.press`:** a declared target activated with a key (`step` 0), or a slider changed by a step (`step` -1 or
  1). `serial` grows with each press; a component reacts in a `changed` handler on it and acts only on its own ids:

```slint
property <int> press-serial: Livi.press.serial;
changed press-serial => { root.pressed(Livi.press.target); }
```

- **Moving:** the arrows and turning the knob move the focus within the zone; Enter or pressing the knob activates
  it (a slider: starts and ends changing it with the knob); `back` goes from the page to the navigation, and on
  LIVI's settings one screen back first; in the navigation, Enter opens what the button stands for and the focus goes
  into the page that opens. The first key with nothing focused enters the page (on the projection without a phone:
  the navigation).
- **LIVI's settings on the plugin's page:** the targets are LIVI's, LIVI activates them itself; the page only draws
  the focus. Their ids: `back`, `apply`, `row:<row>`, `tool:<row>:<i>`, `dialog:<row>`, `pick:<index>`,
  `number-minus`, `number`, `number-plus`, `text`, `color-hex`, `color-reset`, `color-slider:<i>`, `level`,
  `knob:<i>`. Entries of the page's own on the settings' top screen (no way back, `Livi.settings.can-back` false)
  are declared for the page's path as on any page (e.g. `/plugin/settings`): they come first there, in their order,
  then LIVI's, and a press on one reaches the page as `Livi.press`. On deeper screens and while a dialog is open only
  LIVI's count; an empty declaration leaves none.
- **The phone's projection:** while the phone streams, the keys go to the phone as before (CarPlay's knob, Android
  Auto's keys), whatever the plugin declared.
- **Other keys, `Livi.key`:** a key LIVI has no binding for reaches the plugin by its code (`code`, e.g. `"Home"`,
  `"F13"`, as in the key bindings), with `repeat` and a `serial` that grows with each press, on any page; keys bound
  to LIVI's actions stay LIVI's. So a plugin can give extra keys of a car or an adapter a meaning of its own.

The example plugin's page has two buttons for the keys, its bar shows the focus of LIVI's navigation and its settings
page the focus on the settings' rows, with an entry of its own above them on the top screen.

## Companion channel

A program beside LIVI (a voice assistant, a radio, a CAN reader) and the plugin can talk to each other through LIVI's
core, without LIVI knowing what they say:

- The program publishes a small state of its own under a **topic**; the plugin reads it with
  `Livi.extension(topic, key)` as text (`""` when there is none). A key of a nested object is read as `key.inner`; the
  object itself reads as JSON. Numbers read as e.g. `0.5`, booleans as `true` and `false`.
- The plugin sends the program **events** with `Livi.emit(topic, name, value)`; the value is text, `""` when there is
  nothing to say. LIVI hands each event to the programs that listen to the topic at that moment, and to no one
  otherwise; there is no answer.

```slint
// a button of a radio program: hidden while it publishes nothing
if Livi.extension("radio", "station") != "": TouchArea {
    clicked => { Livi.emit("radio", "next", ""); }
}
```

A binding that reads `Livi.extension` runs again whenever any program publishes a change. A program that changes a
value many times a second (e.g. a level) makes the plugin redraw what reads it as often; read such a value in a
`Timer` that runs while it matters, and take slower values over in a `changed` handler.

**On the core socket** (`$XDG_RUNTIME_DIR/livi/core.sock`, frames of 4 bytes of length, little endian, and JSON;
`native/crates/livi-core-proto/src/message.rs`), any client of the user's id can do the program's part:

| Message | Meaning |
|---|---|
| `{"type":"hello","protocol":11,"client":"radio","subscribe":["radio"]}` | The first message; `subscribe` (may be left out) names the topics whose events the client wants. |
| `{"type":"action","id":1,"action":{"kind":"setExtension","topic":"radio","value":{"station":"SRF 3"}}}` | Publishes the client's state under the topic: an object that replaces the one before, at most 4096 bytes as JSON; `null` takes it away. LIVI sends its clients only what changed (patches of `extensions`). |
| `{"type":"event","topic":"radio","name":"next","value":""}` | From LIVI: an event a plugin sent (`Livi.emit`). |
| `{"type":"action","id":2,"action":{"kind":"emit","topic":"radio","name":"next","value":""}}` | Sends an event as a plugin does; the value may be any JSON here. |

- A topic and an event's name have 1 to 64 letters, digits, `-` or `_`. A topic belongs to the client that published
  it first, until that client takes it away or disconnects; a client publishes at most 8. LIVI answers each action with
  a `reply`, with `error` when it refused it (another client's topic, too big, not an object).
- Nothing is saved: when the client disconnects, its topics go from the state, and the plugin reads `""` again. A
  program that starts again publishes its state anew.
- The core checks nothing but the user id of a client. A program that should not reach the rest of LIVI's actions
  needs a gate of its own in between.

## Tools

- `LIVI --check-plugin <folder>` compiles a plugin without a display and reports its errors.
- Preview without a car or phone: `LIVI_UI_STATE=<state.json> LIVI_UI_PLUGIN=<folder> LIVI` runs the UI from a
  recorded state. The files in `native/crates/livi-ui/fixtures/` are examples. Add `LIVI_UI_SIZE=2400x900` for the
  size of your screen and `LIVI_UI_PAGE=plugin/<id>` to open a page.
- `LIVI_UI_TRACE=1` prints how long the steps of the start take, up to the first frames.

## Example

[native/crates/livi-ui/plugin/example](../native/crates/livi-ui/plugin/example) has one page, a bar with a button
per page and a settings page that draws LIVI's settings lists. The tests compile it.

## Pictures as a card

With bars of its own, a plugin can have the phone's picture (CarPlay) shown as a card in the free area between its
bars instead of full screen, like its own pages. The settings (Video, Main Video, Card):

| Setting | Meaning |
|---|---|
| `projectionCard` | the phone's picture as a card; otherwise full screen |
| `videoCardTop`, `videoCardBottom`, `videoCardLeft`, `videoCardRight`, `videoCardRadius` | the card, in pixels of the phone's picture (`projectionWidth` x `projectionHeight`, like the view area); all 0: no card. A card that leaves less than 64 pixels of the picture either way counts as none |
| `projectionWide`, `videoWideTop`, `videoWideBottom`, `videoWideLeft`, `videoWideRight` | a second, wider card, e.g. up to the window's edges on three sides, with the same corner radius; `projectionWide` shows it instead of the card |

- CarPlay knows its view areas from the start, full screen, the card and the wider card if one is set, and draws only in the one it is told. The
  picture keeps its size; CarPlay leaves the rest. Android Auto stays full screen, without the button.
- LIVI keeps the bars and the plugin's background, and leaves the card transparent: the plugin's `background` draws
  around it with `Livi.video` (e.g. a `Path` with `fill-rule: evenodd`). Touches in the card go to the phone, touches
  on the bars stay in LIVI, also where a bar is transparent.
- LIVI draws a button on the picture that steps through card, wider card and full screen until LIVI starts again or
  the setting changes (`Action::SetVideoCard`). Full screen hides the bars; the button leads back.
- `Livi.action("video-view", v)` changes the settings themselves: `card`, `wide` or `full`. `Livi.video.view` says which
  one is set, `Livi.video.wide-set` whether a wider card exists. A background draws around the wider card like around
  the card; with `right` 0 it may leave out the corners on that side.
- Switching moves CarPlay into the other area during the session (`updateViewArea` on the event channel, animated),
  without a new connection. Should the phone refuse it, the session ends and the phone connects anew with the area
  it should be in. A moved card reaches the phone when the settings are applied, like the view area.
- The camera's picture goes full screen below the window, unless the plugin frames the camera page (`frame.camera`
  in the manifest). Its component draws the page (e.g. further cards beside the picture), and with
  `Livi.action("camera-area", …)` it says where the picture goes. LIVI then keeps the bars and the plugin's
  background and reports the area in `Livi.video` like the phone's card, so the same `background` leaves it out; the
  compositor fits the whole picture into it (`videoarea` on its control socket; LIVI's own video view without the
  compositor still shows it full screen). The area stays until the plugin sends another one.
- The picture's rectangle may be larger than the card, also beyond the window: a camera whose picture has black
  borders (e.g. a video converter) gets a rectangle around its card so that only the picture's content lies in the
  card and the borders lie under the background, which leaves out only the card (`Livi.video`).
