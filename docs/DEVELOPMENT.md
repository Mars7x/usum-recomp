# Development and profiling

For Debian/Fedora dependencies, translation, building and normal play, start with
[BUILDING.md](BUILDING.md). This guide covers optional developer checks, replay
validation and experimental renderer switches. Commands identify whether they
run in the source checkout or the generated translation workspace. Keep all
ROM-derived output private.

## Source checks

From the source checkout:

```bash
rustup component add rustfmt clippy
cargo fmt --all -- --check
cargo clippy --locked --all-targets --features usum-runtime/window,usum-runtime/vulkan-compare -- -D warnings
cargo test --locked --features usum-runtime/window,usum-runtime/vulkan-compare
tools/test-check-no-game-data.sh
```

GPU tests marked ignored require a working hardware Vulkan adapter. Ordinary
unit tests do not establish hardware rendering correctness. Run relevant GPU
checks after renderer changes; see the examples below.

## Optional QEMU differential tests

Differential tests of the recompiler (milestone 3) need QEMU user mode
and GNU binutils for ARM, as test tools only. On Debian:

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
The default corpus can require about 12 GB of RAM: run more seeds rather than
larger counts.

Game mode verifies real functions from your own dump (the code is
written only under `target/`; a recorded profile is required):

```bash
USUM_DIFFTEST_DUMP=~/Games/ultra-sun.3ds USUM_DIFFTEST_FUNCTIONS=0x005b9944,0x001178b8 cargo test -p usum-difftest -- --nocapture
```

For Fedora, install the optional test tools and select their names explicitly:

```bash
sudo dnf install qemu-user binutils-arm-linux-gnu
USUM_ARM_AS=arm-linux-gnu-as USUM_ARM_LD=arm-linux-gnu-ld \
USUM_ARM_OBJCOPY=arm-linux-gnu-objcopy \
cargo test -p usum-difftest -- --nocapture
```

Candidate addresses come from `usumctl analyze --leaves <dump>`;
`USUM_DIFFTEST_GAME_CASES` sets the cases per function (2,000). Tool
names can be overridden with `USUM_ARM_AS`, `USUM_ARM_LD`,
`USUM_ARM_OBJCOPY` and `USUM_QEMU_ARM`; the default cross-binutils names are `arm-none-eabi-*`.

## Deterministic probe and replay

In the generated translation workspace, build the probe separately:

```bash
cargo build --release -j 2 -p probe --features usum-runtime/vulkan-compare
./target/release/probe "$usum_rom"
```

`usum_rom` is the absolute dump path set in BUILDING.md. The probe runs a
virtual-clock fixture without the interactive window's real-time throttle.
Successful process exit alone does not mean the requested replay completed;
check the reported stop reason and guest time.

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
| `USUM_GUEST_NATIVE_F32=0` | retain reference single-precision guest arithmetic; the default native path preserves NaNs, denormals and ARM flush-to-zero boundaries through guarded fallback |
| `USUM_GUEST_BLOCK_WORDS=0` | use ordered scalar memory accesses for translated register-list transfers; by default, lists of four or more words share one checked lookup when the entire span is ordinary RAM |
| `USUM_DISPATCH_CACHE=0` | disable the host code-lookup cache for a reference run; enabled by default, with module identity checked on cached module entries |
| `USUM_DISPATCH_FRONT_CACHE=0` | disable the small address cache ahead of the identity-checked hash cache; keep the hash lookup for a same-binary comparison |
| `USUM_DISPATCH_MODULE_REVISION=0` | compare module base/size/name/hash on each cache hit instead of validating the tracked module-list revision; both paths reject changed ownership |
| `USUM_DISPATCH_STATS=1` | log address-cache hits, hash-cache hits and full-resolution misses when a run ends; off by default |
| `USUM_THREAD_LIFETIME_REFERENCE=1` | reproduce the old 32-lifetime-thread limit for diagnostics; normally exited threads release their TLS storage while retained handles keep their original identity |
| `USUM_WAV=file.wav` | record the mixed sound to a WAV file (32,728 Hz stereo) |
| `USUM_DSP_TRACE=n` | describe the first n sound buffers decoded, on stderr |
| `USUM_WATCH=0x1074bc` | report the caller of the last direct call to that address (e.g. the SDK's panic routine) |
| `USUM_VULKAN_ACCELERATE=1` | use guarded Vulkan rasterization; unsupported dependencies or failures rerun safely in software |
| `USUM_DISPLAY_TILE_ROWS=0` | retain per-pixel Morton addressing for common LCD transfers instead of copying groups of eight pixels; same output and guest timing |
| `USUM_VULKAN_DISPLAY_IDENTITY=0` | retain per-pixel scaled LCD source validation; the default uses an exact immutable color-plane owner when available, falling back after CPU writes or alias changes |
| `USUM_VULKAN_BULK_STENCIL=0` | retain scalar stencil extraction from mapped Vulkan readbacks for comparison |
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


## Presentation telemetry

F5 toggles title statistics; `USUM_FRAME_STATS=1` enables them at launch.
`USUM_FRAME_LOG=1` emits approximately one sample per wall second; the session
summary is always logged. Samples distinguish top-screen GSP submissions,
successful host presents, shown/skipped submission versions, repeated presents,
guest-clock/wall-time speed and redraw duration. Present-interval mean/p95 use
the latest 240 completed intervals, including long gaps. Failed presents do
not count. The testing overlay uses submissions for `FPS` and presents for
`WIN`; neither is proof of unique engine frames or physical monitor scanout.
The HUD refreshes even if the producer stops, and those redraws count as repeats.

A numeric `USUM_PRESENT_FPS=1..1000` coalesces window updates without changing
guest timers, audio or the input polling cadence. Resizing/exposing the window
obeys the same cap. Caps above the source update rate do not generate frames.

## Offline builds

In each workspace separately, fetch its dependencies while online, then build
offline with its existing lockfile:

```bash
cargo fetch --locked
cargo build --release --offline --locked
```

The generated workspace creates its own Cargo.lock on its first normal build;
it does not inherit the source checkout's lockfile. Enable the same features and
packages when fetching/building that you intend to use. Flatpak vendoring and
packaging remain unfinished.

## Renderer comparison and diagnostic switches

Build the generated play/probe with `usum-runtime/vulkan-compare` before using
these switches. Newly generated `play` builds include it and default to Vulkan;
`USUM_VULKAN_ACCELERATE=0` selects their software path. The probe remains opt-in:
unset `USUM_VULKAN_ACCELERATE` for software and set it to `1` for acceleration.
For `play`, depth feedback defaults on and `USUM_VULKAN_DEPTH_FEEDBACK=0` disables
it; the probe retains its presence-based diagnostic switch (unset it to disable;
setting it to `0` still enables it). Normal launch
instructions are in BUILDING.md. Unsupported
rendering dependencies and failures retain the guarded software fallback.

Vulkan acceleration enables hardware vertex shaders by default. Set
`USUM_VULKAN_VERTEX=0` to use CPU vertex processing as the reference. The hardware
stage translates supported PICA programs to WGSL and uses indexed GPU draws
without reading shaded vertices back to the CPU. Geometry shaders and unsupported
control flow retain CPU processing; GPU validation failures retain the existing
software fallback. The probe reports offloaded draws/inputs and CPU fallback
counts. R-075–R-077 document a 650-second hardware-vertex comparison with small
rounding differences. Later changes use the replay fixtures listed in
[RESEARCH.md](RESEARCH.md); that historical run is not a claim that every current
build or later game scene has been validated.

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
GPU vertex-input loading uses worker threads at 4096 distinct vertices per draw.
`USUM_VULKAN_INPUT_PARALLEL_MIN=256` restores the previous scheduling threshold
for comparisons. This process-wide threshold is read on first use; values below
256 are clamped. CPU vertex shading and the decoded input bits are unchanged.

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
