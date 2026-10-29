# usum-recomp

An experimental static recompilation project for **Pokémon Ultra Sun** and
**Pokémon Ultra Moon** on Linux. It translates ARM/Thumb game code from your
own decrypted dump into Rust, then compiles it against a Rust runtime for
3DS services, graphics, input, and audio.

## Status — 2026-10-29

**Ultra Sun boots and reaches early gameplay. This is a development project,
not a finished port.** Recorded tests cover the opening, menus, bedroom and
outdoor movement, and early battles. Full-game completion has not been verified.
Ultra Moon is recognized and analyzed, but equivalent gameplay validation
has not been completed.

- Software graphics are the reference renderer.
- Vulkan is the default renderer for `play`, including hardware vertex shaders and GPU window
  presentation, with guarded software fallback. True internal resolution scaling
  supports 1×–8×; 6× renders the top screen at 2400×1440. Rendering correctness
  and performance remain under active development.
- Keyboard, mouse touch and mapped controllers work, including controllers
  connected after launch. Audio, saves and deterministic input replays are
  implemented; sound and compatibility remain incomplete.
- Built-in presentation caps and an optional testing FPS overlay are available.
  Native dynamic FPS is in development.
  Presentation caps do not unlock the game's frame rate. The overlay currently
  measures framebuffer submissions, which do not prove unique rendered frames.
- Recent work fixes area/script callback discovery and reuses exited threads'
  TLS storage while preserving old handles. Individual fixes do not establish
  full-game compatibility. See the [research log](docs/RESEARCH.md#r-119--recycle-exited-thread-tls-without-reusing-thread-identity).
- Flatpak packaging and a graphical launcher remain planned.
- There are no bundled games, translated game executables, or ready-to-play
  downloads. Performance measurements are scene- and hardware-specific.

See [compatibility](docs/COMPATIBILITY.md), [building](docs/BUILDING.md), and
[research evidence](docs/RESEARCH.md).

## Build and inspect

Use a current stable Rust toolchain, Python 3 for the repository data guard,
and Git. Linux is the tested host. Building translated games can require
substantial RAM and disk space; start with a low Cargo job count if needed.

```bash
git clone https://github.com/Mars7x/usum-recomp.git
cd usum-recomp
tools/install-hooks.sh
cargo build --release --locked
cargo test --locked
./target/release/usumctl inspect /path/to/your/ultra-sun.3ds
```

To translate a supported dump locally:

```bash
./target/release/usumctl translate /path/to/your/ultra-sun.3ds
```

Follow [BUILDING.md](docs/BUILDING.md) for Debian/Fedora dependencies, building
the generated `play` program and running the game. Generated files stay
in your private cache and must not be committed or uploaded.

After building `play` in that generated workspace, a 6× testing launch is:

```bash
USUM_RESOLUTION_SCALE=6 USUM_TEST_FPS=1 \
./target/release/play /path/to/your/ultra-sun.3ds
```

F3/F4 change internal resolution, F5 toggles title statistics, F6 cycles the
window presentation cap, and F7 toggles the testing overlay. The overlay is off
by default. See [controls and measurement limits](docs/BUILDING.md#playing-in-a-window).
Use `USUM_VULKAN_ACCELERATE=0` at 1× for the software reference.

## Game-data policy

This repository distributes project source and original synthetic tests,
not game code, assets, keys, firmware, saves, or translated game output.
Supply your own supported decrypted dump. The importer does not decrypt or
download games. Do not attach dumps, saves, screenshots, capture payloads,
or generated game code to issues or pull requests.

The pre-commit hook and CI scan filenames, file contents, generated-code
patterns, and common credential formats. These checks are a backstop, not
a substitute for reviewing what you publish. See
[the data policy](docs/adr/0004-game-data-policy.md).

Pokémon is a trademark of Nintendo, Creatures Inc. and GAME FREAK inc.
This project is not affiliated with or endorsed by them.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for checks, bug reports, and the
restrictions on game-derived material. For vulnerabilities or leaked
credentials, see [SECURITY.md](SECURITY.md).

## Documents

- [Architecture](docs/ARCHITECTURE.md) and [decisions](docs/adr/)
- [Supported inputs](docs/INPUT.md) and [compatibility](docs/COMPATIBILITY.md)
- [Roadmap](docs/ROADMAP.md) and [next steps](docs/NEXT_STEPS.md)
- [Research log](docs/RESEARCH.md) and [feasibility](docs/FEASIBILITY.md)
- [Dependency audit](docs/DEPENDENCIES.md)
- [Developer tests and profiling](docs/DEVELOPMENT.md)

## License

Project code is licensed under [MIT](LICENSE). Dependency licenses remain
with their respective authors. Prior art and reference implementations are
credited in [FEASIBILITY.md](docs/FEASIBILITY.md).
