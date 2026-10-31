# usum-recomp

An experimental static recompiler and Linux host runtime for the Nintendo 3DS
games *Pokémon Ultra Sun* and *Pokémon Ultra Moon*. The games' ARM/Thumb code
is translated ahead of time into Rust and compiled into a native x86-64 Linux
program, which runs on a Rust reimplementation of the console's kernel, system
services, PICA200 graphics, input and audio.

> **This is a development project, not a finished port.** It contains no game
> code, assets or keys, and it cannot run anything without a legally obtained
> copy of the game that you dump and supply yourself.

> **Note:** AI tools were used in developing this project.

## Status

Latest measured state (2026-10-31):

- **Gameplay:** Ultra Sun boots and reaches early gameplay. Recorded replays
  cover the opening, menus, bedroom and outdoor movement, and early battles.
  Full-game completion has not been verified. Ultra Moon is recognized and
  analyzed, but its gameplay has not been validated.
- **Graphics:** Vulkan is the default renderer in `play`, with hardware vertex
  shaders, GPU window presentation and a guarded software fallback. Internal
  resolution scaling goes from 1× to 8× (6× renders the top screen at
  2400×1440). The software renderer remains the reference.
- **Input, audio, saves:** keyboard, mouse touch and mapped controllers
  (including ones connected after launch), audio, saves and deterministic
  input replays work. Sound and compatibility are incomplete.
- **Frame rate:** host presentation caps and an optional FPS overlay exist,
  but they do not unlock the game's frame rate; the overlay counts framebuffer
  submissions, not unique rendered frames. Native dynamic FPS is in
  development: update deadlines, frame publication and elapsed chance checks
  work in private diagnostics, while input sampling and an encounter policy
  still assume 60 Hz ([R-176](docs/RESEARCH.md#r-176-2026-10-31--input-sampling-and-a-down-path-battle-at-variable-cadence)).
- **Not done:** native dynamic FPS, Flatpak packaging and a graphical launcher.

Each run stops at the first operation that is not implemented and names it.
Implementing that operation and replaying the recording is the development
loop. Performance measurements are scene- and hardware-specific.

## How it works

1. **Translation.** The importer validates your decrypted dump and extracts
   the executable and its CRO modules in memory. The recompiler discovers code
   by recursive descent from entry points, calls, jump tables and pointer
   seeds, then emits one Rust function per guest function into generated
   crates in your private cache. The generated code contains no instruction
   decoder.
2. **Runtime.** The native program provides checked guest memory, threads and
   kernel objects (a subset of the 3DS kernel's SVCs), the system services the
   game reaches (file system, HID, GSP, DSP, Y2R, configuration and others),
   module loading and saves.
3. **Graphics and sound.** PICA200 command lists are executed by a Vulkan
   renderer (via wgpu) or the software reference renderer. DSP audio is
   emulated at a high level and played through PulseAudio.

## Building and running

You need current stable Rust, Python 3 (for the repository data guard), Git
and your own decrypted dump. Linux is the tested host. Building translated
games needs substantial RAM and disk space: start with one Cargo job and run
build and test commands one at a time.

```bash
git clone https://github.com/Mars7x/usum-recomp.git
cd usum-recomp
tools/install-hooks.sh
cargo build --release --locked -j 1
cargo test --locked -j 1
./target/release/usumctl inspect /path/to/your/ultra-sun.3ds
./target/release/usumctl translate /path/to/your/ultra-sun.3ds
```

The tools, runtime and synthetic tests build and test without any game files.
See [docs/BUILDING.md](docs/BUILDING.md) for Debian/Fedora dependencies,
building the generated `play` program and running the game. A 6× testing
launch from the generated workspace is:

```bash
USUM_RESOLUTION_SCALE=6 USUM_TEST_FPS=1 \
./target/release/play /path/to/your/ultra-sun.3ds
```

F3/F4 change internal resolution, F5 toggles title statistics, F6 cycles the
presentation cap and F7 toggles the testing overlay (off by default). See
[controls and measurement limits](docs/BUILDING.md#playing-in-a-window). Use
`USUM_VULKAN_ACCELERATE=0` at 1× for the software reference.

## Documentation

- [Building and playing](docs/BUILDING.md)
- [Architecture](docs/ARCHITECTURE.md) and [decisions](docs/adr/)
- [Supported inputs](docs/INPUT.md) and [compatibility](docs/COMPATIBILITY.md)
- [Roadmap](docs/ROADMAP.md) and [next steps](docs/NEXT_STEPS.md)
- [Research log](docs/RESEARCH.md) and [feasibility](docs/FEASIBILITY.md)
- [Dependency audit](docs/DEPENDENCIES.md)
- [Developer tests and profiling](docs/DEVELOPMENT.md)
- [Contributing](CONTRIBUTING.md) and [security](SECURITY.md)

## Legal

This is an independent project, not affiliated with or endorsed by Nintendo,
Creatures Inc., GAME FREAK inc. or The Pokémon Company. Pokémon is a trademark
of its respective owners. The repository contains only original source code,
synthetic tests and documentation. Do not open issues or pull requests
containing game files, keys, firmware, saves, screenshots, capture payloads or
code generated from them; generated translations stay on your machine. The
importer does not decrypt or download games.

A pre-commit hook and CI scan filenames, contents, generated-code patterns and
common credential formats. They are a backstop, not a substitute for reviewing
what you publish. See [the data policy](docs/adr/0004-game-data-policy.md).

Research consulted public emulator and recompiler sources (3dsrecomp, Zakuro,
Citra, 3dbrew); see [FEASIBILITY.md](docs/FEASIBILITY.md) for credits.

Licensed under MIT. See [LICENSE](LICENSE). Dependency licenses remain with
their respective authors.
