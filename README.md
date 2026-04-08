# Larris

**Larris** (laser + Ferris 🦀) is a desktop GUI for GRBL-based laser engravers.
It converts SVG and raster images to GCode, previews the toolpath, and streams
the job directly to the machine over a serial connection — all from one window.

Built with Rust, [Relm4](https://relm4.org/) and GTK 4.

---

## Features

| Tab | What it does |
|---|---|
| **Connect** | Scan serial ports, pick baud rate, connect / disconnect, full console with command entry |
| **Control** | Jog X / Y / Z (0.01 – 100 mm steps), home, feed hold, cycle start, soft reset, status poll, GRBL settings dump |
| **GCode** | Open SVG or raster image, convert to GCode, manage per-layer settings, preview toolpath, stream to machine, abort, save `.gcode` |
| **Preview** | Side-by-side SVG source preview and rendered GCode toolpath |
| **Settings** | Machine dimensions, feedrate, laser power, beam width, begin/end sequences, tolerance, DPI, origin offset |

### Highlights

- **SVG → GCode** conversion with per-layer feedrate, laser power, pass count and mode (outline / fill / default)
- **Raster image** engraving via brightness-to-power mapping (with optional invert for anodised aluminium etc.)
- **Live status bar** — machine state, work position, feed/spindle overrides polled automatically every ~200 ms
- **Safe serial protocol** — `?` sent as a GRBL real-time byte (no trailing `\n`), jog cancel on 0x85, full ok-gated streaming
- **Non-blocking UI** — serial I/O runs on a dedicated OS thread; the GTK main loop is never stalled

---

## Requirements

- Rust 1.80+ (edition 2024)
- GTK 4 development libraries

### Installing GTK 4

**Debian / Ubuntu**
```sh
sudo apt install libgtk-4-dev
```

**Fedora**
```sh
sudo dnf install gtk4-devel
```

**Arch**
```sh
sudo pacman -S gtk4
```

**macOS (Homebrew)**
```sh
brew install gtk4
```

---

## Build & run

```sh
git clone https://github.com/sameer/svg2gcode
cd svg2gcode
cargo run --release
```

Or install to `~/.cargo/bin`:

```sh
cargo install --path .
larris
```

---

## Workflow

1. **Connect tab** — select your port (e.g. `/dev/ttyUSB0`), choose 115200 baud, click **Connect**.
2. **GCode tab** — click **Open** to load an `.svg`, `.png`, or `.jpg` file.
3. Adjust per-layer settings if needed, then click **Convert**.
4. Click **Preview** to inspect the rendered toolpath.
5. Back on GCode — click **Frame** to dry-run the bounding box, then **Send** to stream the job.
6. Monitor progress in the console on the Connect tab; click **Abort** at any time.

---

## Settings reference

| Field | Default | Description |
|---|---|---|
| Begin sequence | `G90 G21 M4` | GCode emitted before the job starts |
| End sequence | `M5 M2` | GCode emitted after the job ends |
| Max X / Y (mm) | 150 / 150 | Machine travel limits (used for preview scaling) |
| Max speed (mm/min) | 10 000 | Upper bound for feedrate |
| Max laser power (S) | 1 000 | S-word ceiling (GRBL `$30`) |
| Beam width (mm) | 0.1 | Hatch line spacing for fill layers |
| Feedrate (mm/min) | 3 000 | Default laser-on feedrate |
| Tolerance (mm) | 0.1 | Bézier linearisation tolerance |
| DPI | 96 | Pixel/point/pica scaling for SVGs without explicit units |
| Laser power (S) | 1 000 | S value written into the begin sequence |
| Origin X / Y (mm) | 0 / 0 | Workpiece offset applied to all coordinates |

---

## Jog step sizes

Use the **+** / **−** buttons on the Control tab to cycle through:
`0.01 mm` → `0.1 mm` → `1 mm` → `10 mm` → `100 mm`

---

## License

MIT — see [LICENSE](LICENSE).