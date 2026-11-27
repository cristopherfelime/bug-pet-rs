# bug-pet-rs

An **unofficial**, lightweight, native Windows desktop-pet runtime written in Rust. It is compatible with the data format and character packs used by *BUG Clicker*, but it is a clean-room runtime: **no original binaries, artwork, character packs, or protection keys are included in this repository.**

Instead of an Electron shell with a web renderer per pet, `bug-pet-rs` is a single native process that draws every pet with Direct3D 11 + DirectComposition, reacts to system audio and typing through native Win32/WASAPI APIs, and idles at a fraction of a percent of one CPU core.

## Benchmark

Measured on the same machine with the same set of pets running side by side, comparing `bug-pet-rs` against the Electron-based BUG Clicker v1.19.8. CPU is the Task Manager "CPU" column; memory is Working Set.

| Metric                       | `bug-pet-rs`      | Electron BUG Clicker v1.19.8 |
| ---------------------------- | ----------------- | ---------------------------- |
| Processes                    | **1**             | 9                            |
| Threads                      | **35**            | 227 – 243                    |
| CPU (Task Manager)           | **0.20% – 0.49%** | 4.90% – 5.58%                |
| Memory (Working Set)         | **~90 – 92 MB**   | 1.26 – 1.38 GB               |
| Render rate                  | 60.1 fps          | —                            |

Numbers depend on hardware, driver, display setup, and the number/complexity of pets; treat them as a reference point, not a guarantee.

## Features

- **GPU rendering** – Direct3D 11 with DirectComposition for per-pixel transparent, click-through-capable overlay windows; rigged characters are skinned on the GPU.
- **GPU adapter selection** – `Auto`, `Hybrid` (low-power adapter drives the pet while compositing stays on the display adapter) and `Ultimate` (high-performance adapter) preferences via `--gpu`.
- **Music reaction** – WASAPI loopback capture with beat detection and **silence gating**: when nothing is playing the audio thread sleeps instead of burning CPU.
- **Typing reaction** – global keyboard activity through `WM_INPUT` (Raw Input); no keyboard hook DLLs and no key content is recorded.
- **Petting** – click/drag interactions with petting particles and squish/swing motion.
- **Lazy overlays** – speech bubbles and the radial action wheel are created on demand and torn down afterwards.
- **Native tray and quick-tweak menus** – plain Win32 popup menus, localized for English, Korean, Japanese and Chinese.
- **Reminders** – built-in reminder bubbles.
- **Safe persistence and auto-sync** – native tweaks are stored separately and never overwrite the original BUG Clicker data (see [Auto-Sync](#auto-sync-with-bug-clicker)).

## Prerequisites and Build

- Windows 10/11 (64-bit) with a Direct3D 11-capable GPU
- Rust (stable) with the **MSVC** toolchain (`x86_64-pc-windows-msvc`) and the Visual Studio Build Tools (C++ workload)

```powershell
git clone <this repository>
cd bug-pet-rs
cargo build --release
```

The executable is produced at `target\release\bug-pet-rs.exe`. Optional tray icons (`tray-32.png`, `tray-16.png`) are looked up in an `assets` folder next to the executable or inside the data directory; a built-in paw icon is used otherwise.

## Setup and Data Discovery

`bug-pet-rs` reads the data folder of an existing BUG Clicker installation (the folder that contains `state.json` and a `characters` directory). You must provide your own data; none is bundled.

The data directory is resolved in this order:

1. `--data-dir <path>` command-line flag
2. `BUG_PET_DATA_DIR` environment variable
3. `%APPDATA%\BUG Clicker\data-location.txt` (a text file written by BUG Clicker containing the data folder path)
4. The default BUG Clicker data folder in your user profile's `Downloads` directory, if present

### Character packs

- **Unencrypted packs** (PNG / WebP / GIF assets with a `character.json`) work out of the box.
- **Encrypted `.bcp` packs** additionally need a *local* transport key that you supply yourself. The key is loaded **strictly into memory**, is never written back, and is never part of this repository. It is searched for in this order:
  1. `--bcp-key <value>` command-line flag
  2. `BUG_PET_BCP_KEY` environment variable
  3. `%APPDATA%\bug-pet-rs\bcp-key.txt`
  4. `<data_dir>\keys\bcp-transport.hex`
  5. `bcp-key.txt` next to the executable

Do not commit or share key material.

## Controls and Quick Tweaks

- **Drag** a pet to move it, **pet** it for particles and reactions, and use the **radial wheel** for quick actions.
- **Tray icon (right-click)**:
  - *Spawn Character* – add any installed character
  - *Pet Quick Settings* – one submenu per active pet: size (±10 px and presets), flip, lock position, click-through, always on top, mute, react to music / typing, OBS key background (transparent / green / blue / magenta), rig-part toggles, close pet
  - *Quick Settings* – music sensitivity, sound mode, audio gain, typing cadence, rest when idle and timeout, bubble font size, lite / reduce motion, language
  - *Gather All*, *Ghost Click-Through*, *Reload from BUG Clicker*, *Launch at Login*, *Quit*
- **Per-pet radial Settings icon** – opens the same per-pet quick-tweak popup directly at the cursor.

### Auto-Sync with BUG Clicker

- On startup and while running, `bug-pet-rs` live-syncs from the original BUG Clicker `state.json`, so changes made in the original app show up automatically (or on demand via *Reload from BUG Clicker*).
- The original data is **kept untouched** by default. Tweaks made in `bug-pet-rs` are saved to its own file at `%APPDATA%\bug-pet-rs\state.json`.
- Use `--read-only` to disable all writes entirely.

## Command-Line Flags

| Flag                      | Description                                                                      |
| ------------------------- | -------------------------------------------------------------------------------- |
| `--data-dir <path>`       | Use this BUG Clicker data directory (must contain `state.json`).                 |
| `--gpu auto\|low\|high`   | GPU adapter preference: automatic/hybrid, low-power, or high-performance.        |
| `--read-only`             | Never write any state to disk.                                                   |
| `--persist`               | Persist native tweaks to `%APPDATA%\bug-pet-rs\state.json`.                      |
| `--bcp-key <value>`       | Local `.bcp` transport key, held in memory only.                                 |
| `--smoke-ms <ms>`         | Run a headless-style smoke pass for the given duration, print stats, and exit.   |
| `--test-beat-ms <ms>`     | Inject a synthetic beat at the given interval for testing reactions.             |

## Legal / Unofficial Disclaimer

`bug-pet-rs` is an independent, unofficial, community project. It is **not affiliated with, endorsed by, or sponsored by** the authors of BUG Clicker. All trademarks, product names, artwork, and character packs belong to their respective owners. This repository contains only original source code: it ships no copyrighted assets, no original application binaries, and no encryption or protection keys. Use it only with data and character packs you are legally entitled to use, and respect the licenses of any content you load.
