# Building (development)

Use the current stable Rust toolchain (validated locally with 1.99.1),
Python 3 for the publication guard, and Git. The workspace declares a
minimum Rust version of 1.88, but that minimum has not been tested against
every locked dependency. The optional window needs the host Wayland/X11
libraries and a PulseAudio-compatible server at runtime; Vulkan needs a
working Vulkan loader/driver. Differential tests need the external tools
below. Flatpak packaging is not yet released.

On Fedora Silverblue, use a toolbox (or rustup in your home directory):

```bash
toolbox create usum && toolbox enter usum
```

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then, in the repository:

```bash
tools/install-hooks.sh
```

```bash
cargo build --release
```

```bash
cargo test
```

```bash
cargo clippy --all-targets -- -D warnings
```

```bash
tools/test-check-no-game-data.sh
```

Differential tests of the recompiler (milestone 3) need QEMU user mode
and GNU binutils for ARM, as test tools only. In an Ubuntu toolbox:

```bash
sudo apt install qemu-user binutils-arm-none-eabi
```

```bash
cargo test -p usum-difftest -- --nocapture
```

They are not part of a plain `cargo test` (the crate is not a default
member), and the build fails, rather than skips, if a tool is missing.
The corpus size is set with `USUM_DIFFTEST_CASES` (random ARM data
processing/multiply/extend, default 20,000), `USUM_DIFFTEST_MEM_CASES`
(loads/stores, 10,000), `USUM_DIFFTEST_THUMB_CASES` (10,000),
`USUM_DIFFTEST_VFP_CASES` (10,000) and `USUM_DIFFTEST_MEDIA_CASES`
(10,000); `USUM_DIFFTEST_SEED` picks a different deterministic corpus.
The default build needs about 12 GB of RAM: run more seeds rather than
larger counts.

Game mode verifies real functions from your own dump (the code is
written only under `target/`; a recorded profile is required):

```bash
USUM_DIFFTEST_DUMP=~/Games/ultra-sun.3ds USUM_DIFFTEST_FUNCTIONS=0x005b9944,0x001178b8 cargo test -p usum-difftest -- --nocapture
```

Candidate addresses come from `usumctl analyze --leaves <dump>`;
`USUM_DIFFTEST_GAME_CASES` sets the cases per function (2,000). Tool
names can be overridden with `USUM_ARM_AS`, `USUM_ARM_LD`,
`USUM_ARM_OBJCOPY` and `USUM_QEMU_ARM`; on Fedora the cross binutils are
`arm-linux-gnu-*`.

Try the CLI on a synthetic image (no game needed):

```bash
./target/release/usumctl make-fixture /tmp/fixture.3ds
```

```bash
./target/release/usumctl inspect /tmp/fixture.3ds
```

With your own decrypted dump (never put it inside the repository):

```bash
./target/release/usumctl inspect ~/Games/ultra-sun.3ds
```

Real-dump tests (read-only on the dumps):

```bash
USUM_DUMP_US=~/Games/ultra-sun.3ds USUM_DUMP_UM=~/Games/ultra-moon.3ds cargo test -p usum-import --test real_dumps -- --ignored
```

## Translating a whole executable

This writes a Cargo workspace of generated crates into your private cache
(never into a git work tree), then builds and runs the probe, which
executes the game's code from its entry point until it needs something
the runtime does not implement yet, and reports what it got through:

```bash
./target/release/usumctl translate ~/Games/ultra-sun.3ds
```

```bash
cd ~/.cache/usum-recomp/translations/ultra-sun-usa-cart && cargo build --release -j 8 -p probe
```

```bash
./target/release/probe ~/Games/ultra-sun.3ds
```

The release build takes about 6 minutes on 16 cores and about 1 GB of
memory per job (RESEARCH.md R-022). Lower `-j` on machines with less RAM.
The workspace is derived from your dump: do not share it.

The probe writes the last frame of each screen to
`target/release/probe-frames/` (top.ppm, bottom.ppm) in the translation
workspace: game-derived images, keep them private.
`USUM_GPU_TRACE=n` prints the first n draws' GPU state on stderr.

The probe runs on a virtual clock, so runs repeat exactly. Its options:

| Variable | Meaning |
|---|---|
| `USUM_SECONDS=n` | stop after n seconds of game time (default 10) |
| `USUM_INPUT="400=A,410=,700=A"` | buttons held from a VBlank on (A B SELECT START RIGHT LEFT UP DOWN R L X Y, and the circle pad pushed fully: CUP CDOWN CLEFT CRIGHT; `Tx:y` touches the bottom screen at pixel (x, y); joined with +) |
| `USUM_FRAMES_EVERY=n` | write both screens every n VBlanks to probe-frames/ |
| `USUM_DUMP=address:length,...` | write those guest bytes (hex) at the end to probe-frames/dump-ADDRESS.bin; an address may follow pointers: `[[6a5518]+18]+c` |
| `USUM_MAX_REFILLS=n` | allow n budget refills (2^20 instructions each; default 16384) before calling it a hang |
| `USUM_SPEED=n` | every n VBlanks, the speed against real time and where the wall time went |
| `USUM_SAVE_FROM=dir` | start from a copy of those saves (to replay a `play` recording) |
| `USUM_PROFILE=first:last` | count guest calls (target, caller) between those VBlanks; reported as `profile` lines |
| `USUM_WAV=file.wav` | record the mixed sound to a WAV file (32,728 Hz stereo) |
| `USUM_DSP_TRACE=n` | describe the first n sound buffers decoded, on stderr |
| `USUM_WATCH=0x1074bc` | report the caller of the last direct call to that address (e.g. the SDK's panic routine) |
| `USUM_VULKAN_ACCELERATE=1` | use guarded Vulkan rasterization; unsupported dependencies or failures rerun safely in software |
| `USUM_VULKAN_DEPTH_FEEDBACK=1` | with acceleration, enable experimental GPU D24S8 feedback conversion; intermediate targets stay on GPU and dependent submissions are queued without CPU waits; final readback still synchronizes. The old `USUM_VULKAN_CPU_DEPTH_FEEDBACK` name remains an alias. |

To read the game's machine code next to the probe's report, extract the
decompressed executable (outside any git work tree) and disassemble it
with GNU binutils:

```bash
./target/release/usumctl extract-code ~/Games/ultra-sun.3ds ~/.cache/usum-recomp/code.bin
```

```bash
arm-none-eabi-objdump -D -b binary -marm --adjust-vma=0x100000 --start-address=0x105bf4 --stop-address=0x105c30 ~/.cache/usum-recomp/code.bin
```

### Playing in a window

The translation workspace also has `play`: the same game in a window,
with keyboard input. It needs a Wayland or X11 session.

```bash
cargo build --release -p play
./target/release/play ~/Games/ultra-sun.3ds
```

| Key (by position) | 3DS |
|---|---|
| arrows | circle pad (the overworld moves with it) |
| I J K L | D-pad |
| X / Z | A / B |
| S / A | X / Y |
| Q / W | L / R |
| Enter / Backspace | Start / Select |
| Escape | quit |
| left mouse button on the bottom screen | touch |

The game runs on the virtual clock, throttled to real time; while the
renderer is slower than the console it plays in slow motion. Sound goes
to the session's PulseAudio (or PipeWire) server; while the game runs
behind real time, the gaps are filled with silence. Saves go to
`$XDG_DATA_HOME/usum-recomp/saves/<program ID>/` (by default
`~/.local/share/...`), apart from the dump and the generated code. Quit
with Escape or by closing the window. The game writes its save data
itself: at first start, and when you save in the game.

`play` records your input as you play. When it exits it writes an input
script to `~/.cache/usum-recomp/recordings/play-<time>.txt`, beside a
copy of the saves it started from (`.save/`). The probe can replay the
session exactly, to reproduce what you saw:
`USUM_SAVE_FROM=<recording>.save USUM_INPUT="$(cat <recording>.txt)"`.

`USUM_PLAY_SNAPSHOT=file.ppm` writes the window's pixels every 60
redraws, a check of what is shown. `USUM_PLAY_SECONDS=n` quits after n
seconds and reports how much sound was played.

The command `usumctl romfs <dump> [offsets...]` lists the RomFS files by
their offset in the RomFS image (the offsets in the probe's `file`
requests); with offsets, only the files holding them.

## Offline builds

`Cargo.lock` is committed and every dependency is from crates.io. For an
offline build: `cargo fetch --locked`, then `cargo build --offline
--locked`. The Flatpak manifest (M6) will vendor sources with
flatpak-builder-tools' cargo generator.

### Building the experimental Vulkan path

In the generated translation workspace, enable the runtime feature explicitly:

```bash
cargo build --release --offline -p probe -p play --features usum-runtime/vulkan-compare
USUM_VULKAN_ACCELERATE=1 USUM_VULKAN_DEPTH_FEEDBACK=1 ./target/release/play /path/to/your/game.3ds
```

Without `usum-runtime/vulkan-compare`, these environment switches do not enable
Vulkan. Depth feedback is experimental; omit its switch to retain the guarded
software fallback for dependent command lists.

For a failing isolated draw, add `USUM_VULKAN_CAPTURE_DIR=/tmp/usum-capture`
to a probe run with `USUM_VULKAN_CAPTURE` and `USUM_VULKAN_SKIP`. Each
`draw-N` directory contains the decoded state, 21 little-endian f32 values
per vertex (position, color, UV0/1/2, quaternion, view), texture RGBA data,
and before/software/Vulkan color and depth buffers. Color is tightly packed
RGBA8; depth is little-endian f32, with dimensions in `state.txt`. These files
are game-derived and must remain private. Export rerenders the captured draw
and is for diagnostics, not timing measurements.
