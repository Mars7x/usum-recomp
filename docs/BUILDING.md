# Build and run on Linux

These instructions cover Debian and Fedora on x86-64. Ultra Sun has gameplay
validation; Ultra Moon has not received equivalent testing. This is an early
source build, with no packaged launcher yet. Use your own supported **decrypted**
dump; see [accepted inputs](INPUT.md). Keep it outside the Git checkout.

There are two builds: first the project tools, then the game workspace generated
from your dump. For tests, profiling and renderer comparison switches, see
[DEVELOPMENT.md](DEVELOPMENT.md).

## 1. Install dependencies

Use a normal Wayland or X11 desktop session. Choose your distribution below.
Commands install build tools and the client libraries used by the play window;
they do not install a desktop environment.

### Debian

```bash
sudo apt update
sudo apt install build-essential git curl ca-certificates python3 pkg-config libudev-dev \
  libwayland-client0 libwayland-cursor0 libxkbcommon0 libxkbcommon-x11-0 \
  libx11-6 libx11-xcb1 libxcb1 libxcursor1 libxrandr2 libxi6 libxext6 libxfixes3
```

For Vulkan, install the loader and diagnostic tools:

```bash
sudo apt install libvulkan1 vulkan-tools
```

For an AMD or Intel GPU using Mesa:

```bash
sudo apt install mesa-vulkan-drivers
```

### Fedora Workstation

```bash
sudo dnf install gcc glibc-devel git curl ca-certificates python3 pkgconf-pkg-config systemd-devel \
  libwayland-client libwayland-cursor libxkbcommon libxkbcommon-x11 libX11 libX11-xcb libxcb \
  libXcursor libXrandr libXi libXext libXfixes
```

For Vulkan:

```bash
sudo dnf install vulkan-loader vulkan-tools
```

For an AMD or Intel GPU using Mesa:

```bash
sudo dnf install mesa-vulkan-drivers
```

On Silverblue/Kinoite, enter a Toolbox and run the Fedora dependency/build
commands there. Keep the same source-checkout path when returning to build:

```bash
toolbox create usum-recomp
toolbox enter usum-recomp
```

The host still supplies the kernel, GPU devices and desktop session.

### GPU, audio and controller requirements

Vulkan needs a driver appropriate for your GPU. For NVIDIA, install your
distribution's appropriate NVIDIA Vulkan driver rather than assuming the Mesa
commands above provide it. Check that Vulkan sees your hardware:

```bash
vulkaninfo --summary
```

A software Vulkan device alone does not validate hardware acceleration. The
runtime also checks required GPU features when it opens the adapter.

Audio uses the desktop's PulseAudio-compatible server. PipeWire works through
its PulseAudio compatibility service (`pipewire-pulse` on Debian,
`pipewire-pulseaudio` on Fedora). Use the server already configured by your
desktop. No ALSA or libpulse development package is needed by this build.

Controllers use gilrs/libudev; the required headers are `libudev-dev` on Debian
and `systemd-devel` on Fedora. SDL2 and SDL3 are not required. Vulkan is loaded
at runtime, so Vulkan SDK/header development packages are not required either.
The package names above follow the official [Debian libudev](https://packages.debian.org/trixie/libudev-dev),
[Fedora systemd](https://packages.fedoraproject.org/pkgs/systemd/systemd-devel/),
[Debian Vulkan](https://packages.debian.org/trixie/libvulkan1) and
[Fedora Vulkan](https://packages.fedoraproject.org/pkgs/vulkan-loader/vulkan-loader/)
and [Fedora Wayland](https://packages.fedoraproject.org/pkgs/wayland/)
package inventories. These are dependency instructions, not a claim of clean-OS
validation on every release or GPU.

## 2. Install Rust

Use current stable Rust through [rustup](https://rust-lang.org/tools/install/).
If rustup is already installed, skip the installer and update stable.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --profile minimal --default-toolchain stable
. "$HOME/.cargo/env"
rustup update stable
rustc +stable --version
cargo +stable --version
```

Run the remaining commands in the same terminal. Rust's linker uses the GCC
build tools installed above. The workspace's declared minimum is not a
validated promise for every locked dependency; current stable is the supported
build recommendation.

## 3. Build the project tools

```bash
git clone https://github.com/Mars7x/usum-recomp.git
cd usum-recomp
tools/install-hooks.sh
cargo +stable build --release --locked -p usumctl
./target/release/usumctl doctor
```

This builds the importer/translator, not the playable game. Python 3 is used by
the publication guard. QEMU and ARM binutils are optional developer test tools;
they are not needed to build or play the game.

## 4. Translate your dump

Set an absolute path to your dump, changing the example to match your file:

```bash
usum_rom="$HOME/Games/Pokemon Ultra Sun.3ds"
./target/release/usumctl inspect "$usum_rom"
./target/release/usumctl translate "$usum_rom"
```

The translator prints its output directory. With the default cache location,
the supported Ultra Sun profile is:

```bash
cd "${XDG_CACHE_HOME:-$HOME/.cache}/usum-recomp/translations/ultra-sun-usa-cart"
```

Use the printed directory if you selected another output/profile. Generated
code is private game-derived data; do not upload it. The generated workspace
references the source checkout by absolute path, so keep that checkout in place.

## 5. Build the game

From the **generated translation workspace**, build the windowed program (Vulkan is included by default):

```bash
cargo +stable build --release -j 2 -p play
```

The first game build is large. Start with two Cargo jobs; use `-j 1` if the
compiler runs out of memory. More jobs can help on machines with enough RAM.
Build time and disk usage depend on the machine and generated code; early
research timings are not current guarantees. Avoid debug builds for gameplay.

This workspace creates its own Cargo.lock on the first build. Keep it for
repeat builds; the source checkout's lockfile does not control this separate
workspace. Repeating the build reuses unchanged generated crates.

## 6. Run the game

### Playing in a window

From that same generated workspace, launch with Vulkan:

```bash
./target/release/play "$usum_rom"
```

Vulkan is the default in newly generated builds and remains experimental.
Hardware vertex processing, depth feedback, deferred display
handling and GPU window presentation are enabled by default when applicable;
unsupported rendering dependencies can fall back to software. To test at 6×
internal resolution (top screen 2400×1440):

```bash
USUM_RESOLUTION_SCALE=6 \
./target/release/play "$usum_rom"
```

For the software reference, disable acceleration explicitly:

```bash
USUM_VULKAN_ACCELERATE=0 USUM_RESOLUTION_SCALE=1 ./target/release/play "$usum_rom"
```

These examples use command-local environment settings. Internal scaling above
1× requires Vulkan; enlarging the window alone does not raise render resolution.
Existing textures and 2D assets retain their source detail.

| Input | Action |
|---|---|
| Arrow keys | Circle pad / movement |
| I / J / K / L | D-pad |
| X / Z | A / B |
| S / A | X / Y |
| Q / W | L / R |
| Enter / Backspace | Start / Select |
| Left mouse button on lower screen | Touch |
| F3 / F4 | Lower / raise internal scale, 1×–8× |
| F5 | Toggle title timing statistics |
| F6 | Cycle window presentation cap |
| F7 | Toggle testing FPS overlay, off by default |
| Escape or close window | Quit |

Controllers can connect before or after launch. Left stick maps to the circle
pad; D-pad maps directly. Xbox A/B/Y/X map to 3DS A/B/X/Y, bumpers to L/R, and
Start/Select directly. Keyboard and controller input work together. Disconnect
and focus loss release controller input.

The game attempts normal speed; an overloaded host can run behind and produce
audio gaps. `USUM_PRESENT_FPS=auto` is the default; a numeric value limits host
presents without unlocking the game. Native dynamic FPS remains in development.
The optional overlay (`USUM_TEST_FPS=1`, or F7) reports GSP framebuffer submissions
as `FPS` and window presents as `WIN`; neither proves unique engine frames.

## Saves and generated files

| Data | Default location |
|---|---|
| Generated workspace/build | `~/.cache/usum-recomp/translations/<profile>/` |
| Game saves | `~/.local/share/usum-recomp/saves/<program ID>/` |
| Recorded input and starting-save copies | `~/.cache/usum-recomp/recordings/` |

`XDG_CACHE_HOME` and `XDG_DATA_HOME` override the corresponding base directories.
The original dump is read in place. Back up game saves separately from generated
code and build files. Recordings, saves and captures are private game data too.

## Updating an existing build

Return to your source checkout, then rebuild the translator and regenerate:

```bash
cd /path/to/usum-recomp
git pull --ff-only
cargo +stable build --release --locked -p usumctl
./target/release/usumctl translate "$usum_rom"
cd "${XDG_CACHE_HOME:-$HOME/.cache}/usum-recomp/translations/ultra-sun-usa-cart"
cargo +stable build --release -j 2 -p play
```

Set `usum_rom` again if this is a new terminal. Regeneration picks up discovery
and translation changes; rebuilding alone is insufficient for those changes.
Unchanged generated files retain their timestamps. Rebuilding does not delete
the separate game-save directory.

## Troubleshooting

| Symptom | Check |
|---|---|
| `pkg-config` or `libudev` build error | Install the distribution's build dependencies in the same host/container where Cargo runs. `pkg-config --modversion libudev` should succeed. |
| `linker cc not found` | Install `build-essential` (Debian) or `gcc`/`glibc-devel` (Fedora). |
| Compiler killed / out of memory | Retry the release build with `-j 1`; close other memory-heavy builds. |
| No Vulkan adapter / driver error | Check `vulkaninfo --summary` in the same environment; verify the hardware driver and required features. Software rendering is available at 1×. |
| Scaling requested without acceleration | Build with the Vulkan feature and set `USUM_VULKAN_ACCELERATE=1`, or return to scale 1. |
| Window cannot open | Run from your graphical desktop session with its normal Wayland/X11 environment. |
| No sound | Check the desktop's PulseAudio/PipeWire compatibility service and session socket. |
| Controller not detected | Check normal session access to `/dev/input/event*`; replug the pad. Container access also depends on host device permissions. |
| Missing source dependency path after moving checkout | Rebuild `usumctl` and rerun translation from the new checkout location. |
| Unsupported input / encrypted dump | See [INPUT.md](INPUT.md); the importer does not decrypt or download games. |

For reproducible bug reports and developer diagnostics, see
[CONTRIBUTING.md](../CONTRIBUTING.md) and [DEVELOPMENT.md](DEVELOPMENT.md).
