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
| `USUM_GUEST_PAGE_INDEX=0` | disable the checked full-page region index for a reference run; partial pages always use the original region search |
| `USUM_GUEST_BLOCK_WORDS=0` | use ordered scalar memory accesses for translated register-list transfers; by default, lists of four or more words share one checked lookup when the entire span is ordinary RAM |
| `USUM_DISPATCH_CACHE=0` | disable the host code-lookup cache for a reference run; enabled by default, with module identity checked on cached module entries |
| `USUM_THREAD_LIFETIME_REFERENCE=1` | reproduce the old 32-lifetime-thread limit for diagnostics; normally exited threads release their TLS storage while retained handles keep their original identity |
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

| Key (by position) | Action |
|---|---|
| arrows | circle pad (the overworld moves with it) |
| I J K L | D-pad |
| X / Z | A / B |
| S / A | X / Y |
| Q / W | L / R |
| Enter / Backspace | Start / Select |
| Escape | quit |
| F3 / F4 | lower / raise Vulkan internal resolution scale (1×–8×) |
| F5 | show/hide frame statistics in the window title |
| F6 | cycle the host presentation cap: auto / 30 / 60 / 120 / 144 / 240 |
| F7 | toggle the top-left testing FPS counter (off by default) |
| left mouse button on the bottom screen | touch |

Frame timing is built into the runtime. `USUM_PRESENT_FPS=auto` (the default)
adds no presentation cap; a number from 1 to 1000 limits window presents per
second. For example, `USUM_PRESENT_FPS=30 ./target/release/play game.3ds` caps
the window while the guest clock, game rendering, audio and input scheduling
continue normally. A cap above the game's update rate does not generate extra
game frames. This is a maximum, not a guaranteed rate; source timing or host
load can leave it underfilled. Pending window updates coalesce; resizing/exposing the window uses
the same cap. Input-device polling retains its own cadence.

The title separates GSP top-screen framebuffer **updates/s**, successful host
**presents/s**, and **game speed** (guest-clock advance divided by wall time).
Updates are framebuffer submissions, not proof of unique images; presents are
window-system submissions, not physical monitor scanout. These measurements do
not establish uncapped gameplay FPS. Frame-interval p95 covers the latest 240
completed present intervals, including long gaps. `USUM_FRAME_STATS=1` enables
title statistics at launch; they are otherwise hidden until F5 is pressed.
`USUM_FRAME_LOG=1` writes samples approximately
once per wall second; an end-of-session summary is always logged. Samples also
report shown update versions, repeated presents, skipped update versions between
shown frames, and mean host redraw time (including waits). Failed presents do not count.
Guest-clock samples come from the latest published VBlank; session averages
include frontend startup, while the periodic rates use their measured interval.
These controls are frontend presentation policy, independent of guest timers;
game-side higher-FPS support remains future work.

The separate top-left testing counter is off by default. Press F7 or launch
with `USUM_TEST_FPS=1` to enable it. Its `FPS` line uses game top-screen updates
per wall second; `WIN` shows successful window presents per wall second. The
framebuffer-submission caveat above applies to `FPS`: it is not proof of unique
rendered images. The panel is composited only onto the host window, independently
of internal resolution, and never written into guest images or saves. Both
Vulkan and CPU presentation support it. Its small bitmap/upload is cached until
the one-second sample changes; disabling it skips HUD drawing/upload work.

Controllers are detected automatically, including controllers connected after
launch. A newly connected pad or the most recently used pad becomes active.
Left stick controls the circle
pad with a radial dead zone and proportional movement; D-pad controls the 3DS
D-pad. South/East/North/West face buttons map to A/B/X/Y (Xbox A/B/Y/X or
PlayStation Cross/Circle/Triangle/Square); bumpers map to L/R and Start/Select
map directly. Keyboard and controller inputs can be used together. Input is
neutralized while the window is unfocused, and disconnecting releases the pad.

On Linux, building the window feature now needs `pkg-config` and `libudev-dev`
(Debian/Ubuntu; Fedora uses `pkgconf-pkg-config` and `systemd-devel`). Controller
access uses the normal desktop session's input-device permissions. Linux
containers automatically select device-node notifications so late connections
and permission changes can be detected without host udev broadcasts. Failed
backend initialization is retried; an empty but healthy backend stays running.

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
copy of the saves it started from (`.save/`). Analog stick positions are recorded
as `C<x>:<y>` tokens (each axis -156..156); old digital recordings remain valid.
The probe can replay the
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

Vulkan acceleration enables hardware vertex shaders by default. Set
`USUM_VULKAN_VERTEX=0` to use CPU vertex processing as the reference. The hardware
stage translates supported PICA programs to WGSL and uses indexed GPU draws
without reading shaded vertices back to the CPU. Geometry shaders and unsupported
control flow retain CPU processing; GPU validation failures retain the existing
software fallback. The probe reports offloaded draws/inputs and CPU fallback
counts. The full 650-second recording is validated with very small rounding
differences; the renderer remains experimental and broader gameplay is unverified.

Vertex programs whose maximum loop counts fit the 65,536-instruction limit prove
that budget once at compilation. Larger programs keep per-draw checks using the
actual loop uniforms. `USUM_VULKAN_STATIC_BUDGET=0` restores per-draw checking for
all programs; this process-wide policy is captured at the first successful shader
compilation so cached programs use one policy.

Hardware draws with at least 16 distinct vertices precompute attribute
destinations and fixed input registers once per draw. All loader reads retain
their order and checked-memory fallback; CPU shading keeps the reference loader.
`USUM_VULKAN_INPUT_PLAN=0` restores that loader for GPU draws too. The probe reports
planned draws and distinct inputs separately from total GPU vertex counts.

GPU vertex uploads pack each complete 256-byte input as one block while preserving
native-endian float bits. `USUM_VULKAN_BLOCK_PACK=0` retains the scalar float-by-float
packer; the process captures this policy at its first hardware vertex upload.

The synthetic hardware vertex checks use no game data:

```bash
cargo run -p usum-runtime --example vertex-shader-smoke --features vulkan-compare
```

`USUM_RESOLUTION_SCALE=1..8` selects the Vulkan internal resolution multiplier
(native is 1). For example, `USUM_RESOLUTION_SCALE=6` renders the top screen at
2400×1440. F3/F4 lower/raise the multiplier while playing; the window title shows
the requested scale. Window resizing controls how the two screens fit on your
display and is independent of internal rendering resolution. Touch coordinates
remain native. Unsupported/CPU-rendered presentation uses native output.

Scaled color/depth targets and viewports are actually rasterized at the higher
resolution. Guest buffers retain native dimensions; a full-resolution host image
survives supported LCD transfers and rotation into the play window. Native guest
snapshots remain suitable for comparison. Compatible targets use immutable
readback tickets and frozen framebuffer textures by default at native and
scaled resolutions.
Use the same internal scale for gameplay and probe performance measurements.

Scaled targets now accept native-size color/depth uploads, with exact replication
performed on the GPU. Native guest readbacks likewise sample the scaled target
on the GPU; full-resolution color stays resident through supported LCD transfers
and Vulkan window presentation.
`USUM_VULKAN_SCALED_UPLOAD=0` and `USUM_VULKAN_NATIVE_READBACK=0` select the older
CPU expansion/full-readback paths for reference comparisons. Display cropping,
rotation and RGB packing also execute on the GPU;
`USUM_VULKAN_GPU_DISPLAY_COPY=0` restores the CPU display transform.

Scaled LCD transfers freeze immutable packed RGB buffers on the GPU. The Vulkan
window samples those buffers directly with the CPU presenter's integer nearest
sampling, layout and clipping. Captures and the software window fallback request
an exact, cached CPU copy on demand. Unsupported window surfaces automatically
use software presentation. `USUM_VULKAN_RESIDENT_DISPLAY=0` restores eager RGB
readback, and `USUM_VULKAN_GPU_PRESENT=0` selects the software window compositor;
set both to `0` to compare the previous display path.

`USUM_PLAY_REPLAY=1` runs a bounded window replay from `USUM_INPUT` /
`USUM_INPUT_FILE` and exits at `USUM_SECONDS`, with normal virtual-clock pacing
retained. It uses the normal play save directory: automated comparisons set
`XDG_DATA_HOME` to an isolated directory containing the starting save fixture.
Window redraws and distinct resident buffers are diagnostic counts, not proof of
unique game frames or uncapped normal-speed FPS.

The synthetic resize check opens and closes its own window while a second thread
submits raster work. It needs no game data or manual input:

```bash
cargo run --release -p usum-runtime --example display-window-smoke --features window,vulkan-compare
```

Compatible RGBA8/D24S8 targets with depth buffers use immutable readback tickets
by default. Exact guest plane identities guard resident target reuse; CPU writes
invalidate reuse. Actual CPU readers receive the frozen result, including
stencil bytes. Other formats keep synchronous writeback. Deferred device failures
become guest memory faults rather than stale RAM. The probe reports queued and
materialized tickets and materialization time separately.

CPU reads encode only the requested color or depth/stencil plane into guest
layout. Both planes still come from the same immutable GPU ticket, and reading
both counts as one materialized target. `USUM_VULKAN_SPLIT_ENCODE=0` restores
eager conversion of both planes for reference comparisons.

D24S8 imports recognize complete, exactly uniform depth/stencil planes and fill
their linear buffers with the decoded value. Nonuniform planes retain the tiled
decoder. `USUM_VULKAN_UNIFORM_DEPTH=0` selects that decoder for every import.

Full-plane framebuffer color and depth/stencil textures also use immutable GPU
views. Matching dimensions, format and logical device-write identity are required;
CPU writes, subviews and incompatible layouts use ordinary texture decoding.
Native diagnostics can request `TextureSnapshot::resolved_pixels()` on demand.
Both paths preserve default-on outlines and full internal-resolution detail.

`USUM_VULKAN_DEFER_READBACK=0` selects synchronous readback;
`USUM_VULKAN_DEFER_TEXTURES=0` disables frozen GPU texture captures while retaining
deferred target writes. These options now use `0` to disable and are enabled
when unset. `USUM_VULKAN_NATIVE_READBACK=0` also disables scaled deferred targets
so the older full-readback reference remains usable. All reference switches can
be combined with `USUM_VULKAN_VERTEX=0` for CPU vertex shading.

Common unfiltered RGBA8-to-RGB8 LCD transfers read only the prefix containing
sampled tiles, omitting unused padded rows/columns. `USUM_DISPLAY_FULL_INPUT=1`
restores the advertised full input extent for same-binary reference checks.
Other transfer modes keep their previous full input extent.

Synthetic deferred-readback checks use no game data:

```bash
cargo run -p usum-render --example deferred-readback
cargo test -p usum-runtime --features vulkan-compare deferred::tests -- --ignored
```

The probe reports target reuse/uploads, capture preparation, final submit/readback,
and CPU guest-buffer writeback times. Submit/readback includes mapping and host
copies, so it is not a measurement of GPU execution alone. `USUM_SPEED=1800`
also reports interval speed and combined CPU vertex loading/shading time during
a long replay. Compare thread settings with `RAYON_NUM_THREADS=n` using the same
recording and initial saves; repeat samples in reverse order. Wait for video
encodes and other heavy background jobs before drawing FPS conclusions.
The `game frames` report counts top-screen framebuffer updates and gives rates
against both wall time and the game clock; window redraw counts are a separate
presentation statistic. Unthrottled probe throughput does not establish an
uncapped frame rate at normal game speed.

For a replay comparison, run software and Vulkan with the same input recording,
initial saves, duration and `USUM_FRAMES_EVERY` interval. Copy the probe executable
into a separate private directory for each run: captures are written into
`probe-frames` beside the executable, so reusing it would overwrite the reference.
For long runs, also raise `USUM_MAX_REFILLS`: the 650-second recording needs
more than the default 16,384 refills; `USUM_MAX_REFILLS=65536` is sufficient.
Check the final stop reason and game time, since a successful process exit alone
does not mean the requested duration was reached.
From the source checkout, compare the resulting directories:

```bash
python3 tools/compare-probe-frames.py /private/software/probe-frames /private/vulkan/probe-frames
```

The JSON report contains RGB mean absolute error (0–255), maximum channel error,
and pixel counts with any channel error greater than eight (adjustable with
`--threshold`). It matches exact filenames and reports missing frames; missing
or malformed captures cause a nonzero exit status. Pixel differences alone do
not imply an automatic pass or failure. Use `--pattern '??????-*.ppm'` to compare
only periodic frames (quiet runs omit the final `top.ppm` and `bottom.ppm`).
Keep captures private and inspect the
important scenes alongside these measurements.

For a failing isolated draw, add `USUM_VULKAN_CAPTURE_DIR=/tmp/usum-capture`
to a probe run with `USUM_VULKAN_CAPTURE` and `USUM_VULKAN_SKIP`. Each
`draw-N` directory contains the decoded state, 21 little-endian f32 values
per vertex (position, color, UV0/1/2, quaternion, view), texture RGBA data,
and before/software/Vulkan color and depth buffers. Color is tightly packed
RGBA8; depth is little-endian f32, with dimensions in `state.txt`. These files
are game-derived and must remain private. Export rerenders the captured draw
and is for diagnostics, not timing measurements.
