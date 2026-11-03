# Debugging LIVI

How to turn on diagnostics and where to read the logs when something misbehaves.

## Process layout

LIVI runs as several processes, which is why the flags below take effect in
different places. Every flag is an environment variable, and the child processes
inherit whatever you set before the LIVI command:

```
LIVI_GST_DEBUG=1 ./LIVI*.AppImage
```

- **core** is `livi-core`. It drives USB, sessions, the projection drivers and the
  settings, and starts the other processes.
- **UI** is the `LIVI` window (Slint). On Linux core starts it, on macOS it starts
  core. It talks to core over a unix socket.
- **gst-host** is a standalone native binary that runs the GStreamer video
  pipeline. Core spawns it and talks to it over a unix socket.
- **livi-compositor** is LIVI's own Wayland compositor on Linux. Core spawns it.
- **helper** is `livi-helperd`, a native binary core runs as root. It sets up
  Bluetooth, Wi-Fi and the access point for native Android Auto and CarPlay.

## Where the logs go

- **stdout / stderr**: everything is printed to the console, timestamped. When LIVI
  is started from the autostart entry, the output is redirected to `LIVI.log` next
  to the AppImage.
- **gst-host crash backtrace**: on a pipeline crash the child writes
  `livi-gst-host-crash.log` next to the AppImage (or the working directory), and
  core prints it as well.

## Debug flags

### General

| Flag | Effect |
| --- | --- |
| `DEBUG=1` | General debug logging. Also enables the helper debug flag. |
| `TRACE=1` | Trace level logging. |

### Video and GStreamer

| Flag | Effect |
| --- | --- |
| `LIVI_GST_DEBUG=1` | GStreamer debug logging with Pi-tuned categories (`v4l2codecs-decoder:6,v4l2codecs-h265dec:6,waylandsink:5,wl_dmabuf:6`). Any other value is used verbatim as the GStreamer debug string, for example `LIVI_GST_DEBUG="waylandsink:6"` or `LIVI_GST_DEBUG="*:5"`. |
| `LIVI_GST_SWDEC=1` | Force software decoding (avdec_*), bypassing the hardware decoder. Use it to tell a broken hardware decoder apart from a broken stream. |
| `LIVI_GST_SINK=<element>` | Override the video sink (default `waylandsink` on Linux). |
| `LIVI_GST_PRELOAD=<lib.so>` | LD_PRELOAD a library into the gst-host child only. |

### Compositor and Wayland

| Flag | Effect |
| --- | --- |
| `LIVI_WLR_DEBUG=1` | Raise the nested wlroots compositor log level to `WLR_DEBUG`. |
| `LIVI_DEBUG_BG=1` | Paint a magenta debug background in the compositor, to see the layout and video planes. |
| `LIVI_NO_COMPOSITOR=1` | Start without the nested compositor. |
| `LIVI_COMPOSITOR_EARLY_CALLBACK=1` | Display backend: frame callbacks when the GPU starts a frame, not when it has finished it, so that a client's next frame overlaps the GPU's work. More frames per second from a slow client, up to one frame more latency. |
| `LIVI_COMPOSITOR_UI_HOLE=1` | Leave the UI out over a video where the UI says it draws nothing: the card without what LIVI and the plugin report over it (docs/plugins.md, "Over a video"). The compositor offers the protocol `livi-transparent-region-v1` (native/protocols) only then; the UI tells it with each frame. `[hole]` lines in the compositor's log and `[ui] hole:` in the UI's say what is left out when it changes. |
| `LIVI_COMPOSITOR_INVALIDATE=1` | A frame that draws everything anew starts with `glInvalidateFramebuffer`, so that a tile-based GPU need not load the old contents. `clear` starts it with a `glClear` to magenta instead, which shows if a frame leaves a pixel out. |

### UI

| Flag | Effect |
| --- | --- |
| `LIVI_NO_UI=1` | Run livi-core without starting the UI. |
| `LIVI_UI_CMD=<command>` | Start this shell command as the UI instead. |
| `LIVI_UI_SIZE=<w>x<h>` | Open the UI window at this size, e.g. `320x200`. |
| `LIVI_UI_PAGE=<page>` | Open the UI on `projection`, `media`, `telemetry`, `camera`, `settings` or a plugin's page, `plugin/<id>`. Empty counts as not set. |
| `LIVI_UI_HOLE_CHECK=1` | Read each frame back after drawing it and log every pixel the UI declares transparent for `LIVI_COMPOSITOR_UI_HOLE` that is not (`[ui] hole check:`). Also without LIVI's compositor, e.g. in a preview. Costly: one readback a frame. |
| `LIVI_UI_PLUGIN=<folder>` | Load the UI plugin from this folder instead of `~/.config/LIVI/ui-plugin` (`docs/plugins.md`). `LIVI --check-plugin <folder>` only compiles it and reports its errors. |
| `LIVI_UI_TRACE=1` | Print how long the steps of the UI's start take, up to the first frames. |
| `LIVI_DEBUG_OVERLAY=1` | Show the debug overlay (frame rate, load, memory, page, plugin) in a corner, like the setting *System → Logs → Debug overlay*. |
| `LIVI_UI_SETTINGS=<path>` | With the settings page (LIVI's or a plugin's, `plugin/<id>`), open this settings path, e.g. `general/connections`. |
| `LIVI_UI_SNAPSHOT=<file.png>` | Save a picture of the UI window 3 s after start, `LIVI_UI_SNAPSHOT_MS` sets another wait. |
| `LIVI_UI_STATE=<file.json>` | Show this core state without a core, e.g. `native/crates/livi-ui/fixtures/dab.json`. |
| `SLINT_SKIA_PARTIAL_RENDERING=1` | Slint's partial rendering: the UI window redraws only what changed instead of the whole window each frame. Needs the Skia renderer on OpenGL (build feature `skia` of `livi-ui`) and a driver with `EGL_EXT_buffer_age`; without it Slint quietly redraws everything. `log` also prints the share of the window each frame repaints, `visualize` outlines it on screen. Slint turns it off for a window with a rendering notifier; LIVI's main window has none (except on macOS, where it gains nothing). |
| `SLINT_DEBUG_PERFORMANCE=refresh_lazy,console` | Print the UI's frame rate once a second (Slint's own counter). |

The `SLINT_*` variables are Slint's own and are read by the UI. Set them like the
other flags, before the LIVI command (core passes them on to the UI), or in a
system service as `Environment=SLINT_SKIA_PARTIAL_RENDERING=1` in its unit.

### CarPlay, Bluetooth and Wi-Fi helper

| Flag | Effect |
| --- | --- |
| `LIVI_CP_DEBUG=1` | Verbose CarPlay and helper logging (also enabled by `DEBUG=1`). |
| `LIVI_CP_SYSLOG=1` | Tap the CarPlay helper output into syslog. |
| `LIVI_BT_AGENT=0` | Leave Bluetooth pairing to another program on the system (e.g. one that shows the code and asks the user): LIVI registers no pairing agent and does not touch the controller's `Pairable`, also not after `LIVI_BT_PAIRING_SECONDS` or when it exits. Discovery for the wireless connections stays LIVI's. Without it, LIVI's agent accepts every pairing. |

## Common workflows

**Video does not show up.** Look at the hardware path and its log first, then
isolate against software decode:

```
LIVI_GST_DEBUG=1 ./LIVI*.AppImage        # what the decoder and sink report
LIVI_GST_SWDEC=1 ./LIVI*.AppImage        # does software decode work at all
```

If software decode works and hardware does not, the hardware decoder or the GPU
behind it is the problem, not the stream.

**Touch or layout problems, or a black video plane.**

```
LIVI_WLR_DEBUG=1 ./LIVI*.AppImage        # compositor, seat and input events
LIVI_DEBUG_BG=1 ./LIVI*.AppImage         # make the plane layout visible
```

**CarPlay, Bluetooth or Wi-Fi bring-up.**

```
LIVI_CP_DEBUG=1 ./LIVI*.AppImage
```
