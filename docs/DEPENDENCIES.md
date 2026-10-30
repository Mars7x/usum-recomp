# Dependency audit

Policy: project code is Rust. Dependencies must be Rust crates. Any crate
that compiles or links C/C++ (a `-sys` crate, `cc`/`cmake` in a build
script, a `links` key) must be listed here with the reason and the
conflict explained. Interfaces to the OS and the graphics driver
(libc via std, the Vulkan loader, ALSA/PipeWire, udev) are acceptable
but still listed.

## Dependency inventory (updated 2026-10-29)

The original CLI audit used `cargo tree -e normal,build` on 2026-10-25.
The table and platform exceptions below also include the subsequently added
window, Vulkan and controller features; this is not a security advisory scan.

| Crate | Why | Native code? |
|---|---|---|
| sha2 0.10 (`default-features = false, features = ["std"]`) | SHA-256 for integrity checks and profiles | No. The optional `asm` feature (C/asm via sha2-asm) is off. Uses `cpufeatures` for runtime CPU detection with Rust intrinsics |
| digest, block-buffer, crypto-common, generic-array, typenum, cpufeatures, cfg-if | sha2's own dependencies | No |
| serde, serde_derive (+ syn, quote, proc-macro2, unicode-ident) | Serializing reports (`--json`) | No (proc-macro at build time) |
| serde_json (+ itoa, memchr, zmij) | JSON output | No |
| version_check | build-time check used by generic-array | No |
| rayon 1 (+ rayon-core, crossbeam-deque, crossbeam-epoch, crossbeam-utils, either) | A persistent worker pool for the renderer's parallel bands and vertex shading (R-037): no thread is started per draw, and borrowed draw state is shared without `unsafe` in our code | No |
| wgpu 30.0.1 (`default-features = false`, features `std vulkan wgsl`) + naga 30.0.1 | The isolated `usum-render` host-GPU backend; WGSL validation/translation and Vulkan command submission | No compiled C. `ash` loads the OS Vulkan driver at run time; the selected Linux backend is Vulkan only |
| pollster 1.0.1 | Blocks on wgpu's adapter/device and validation futures during native bring-up | No |
| bytemuck 1.25.2 | Safe borrowed byte views of padding-free vertex input arrays; preserves float bit patterns | No |
| gilrs 0.11.2 + gilrs-core 0.6.8 (window feature) | Standard controller mappings, analog sticks and hot plugging | Linux links the OS libudev through libudev-sys; pkg-config/libudev development files are required at build time. OS interface exception; no bundled C/C++ gamepad implementation. Windows uses the OS Windows Gaming Input interface |

For the original CLI-only dependency set audited at that checkpoint, no
crate had a `links` key or a `-sys` suffix. The optional window and Vulkan
backends have additional platform dependencies; see the sections below. Our crates forbid `unsafe` (`unsafe_code = "forbid"`). Some
dependencies (including bytemuck, memchr, serde_json and typenum) contain `unsafe`
internally, as most of the Rust ecosystem does. This is disclosed, not
avoided.

## The play window (usum-runtime feature `window`, 2026-10-26)

The runtime's window/Vulkan features are off by default for tools and ordinary
tests. The generated `play` program requests both features and uses Vulkan by
default; the probe can enable Vulkan explicitly for comparisons.

| Crate | Why | Native code? |
|---|---|---|
| winit 0.30 (`default-features = false`, features `rwh_06 x11 wayland wayland-dlopen`) | The window, keyboard events, the event loop | No compiled C. Talks to Wayland through wayland-client/wayland-backend and to X11 through x11-dl and x11rb. With the `dlopen` features, `libwayland-client`, `libxkbcommon`, `libX11`, `libXcursor`, `libXrandr`, `libXi` are opened at run time (dlib/libloading): client libraries of the OS's window system |
| softbuffer 0.4 (`default-features = false`, features `x11 x11-dlopen wayland wayland-dlopen`) | Presents CPU-drawn pixels in the window | No compiled C. tiny-xlib opens `libX11`/`libX11-xcb` at run time |
| smithay-client-toolkit, calloop, wayland-* , x11rb(-protocol), xkbcommon-dl, xkeysym, rustix, polling, memmap2, and about 60 others (`cargo tree -p usum-runtime --features window`) | winit's and softbuffer's own dependencies | No compiled C (see below) |

`cc` and `pkg-config` appear as build dependencies (of wayland-backend,
wayland-sys and tiny-xlib) but do nothing in this configuration:
- wayland-backend compiles its C log shims only with its `log` feature,
  which is off. The build directory has no object files (checked).
- With the `dlopen` features, wayland-sys and tiny-xlib do not use
  pkg-config to link anything.
For these window-system dependencies, `wayland-sys` and `x11-dl` only declare
symbols looked up at run time. The controller backend additionally links the
OS libudev through `libudev-sys`, as disclosed above.

| pulseaudio 0.3 (+ mio, futures, byteorder, num-traits, enum-primitive-derive, thiserror) | Sound: the PulseAudio native protocol implemented in Rust, over the session's socket (PipeWire's pulse server speaks it too) | No. No client library is linked or opened: it talks the protocol on the Unix socket itself. In a Flatpak this needs `--socket=pulseaudio` |

cpal was the expected choice. It was not taken: its Linux backend links
libasound through alsa-sys, which needs ALSA's development files at build
time, and the native protocol needs no C at all (R-039).

Gamepads are implemented with gilrs/libudev, including hot plugging. SDL2 and
SDL3 are not dependencies of the current input/window stack.

## Test-only tools (M3)

Used by `crates/usum-difftest`'s build script, which is excluded from the
workspace's default members. They are external programs, never linked into
or shipped with the product. They are C, which does not conflict with the
requirement because they are not part of the product.

| Tool | Version used | Role |
|---|---|---|
| `qemu-arm` (QEMU user mode, `qemu-user` package) | 8.2.2 (Ubuntu 24.04) | Independent reference: runs the original programs on its `arm11mpcore` model |
| `arm-none-eabi-as`, `-ld`, `-objcopy` (`binutils-arm-none-eabi`) | 2.42 | Independent encoder: assembles test programs and random instruction text, so encodings never come from our decoder |

`usum-difftest` itself has no dependencies beyond `usum-guest` (and
`usum-recomp` at build time).

## Expected later (not yet added)

| Need | Likely crate | Native dependency to disclose |
|---|---|---|
| Launcher UI | gtk4-rs/libadwaita or a Rust-native toolkit | GTK is C; a conflict to decide in an ADR at M6 |
