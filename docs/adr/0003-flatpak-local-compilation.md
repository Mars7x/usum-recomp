# ADR-0003: Compiling the user's translated code inside the Flatpak

- Status: proposed (prototype in milestone 6)
- Date: 2026-10-25

## Context

The translated Rust is derived from the user's dump, so it cannot be
shipped. It must be generated *and compiled* on the user's machine. A
Flatpak app normally runs on a *Platform* runtime, which has no compiler.

Checked on 2026-10-25 (`flatpak remote-info flathub ...`, see RESEARCH.md
R-008):

- `org.freedesktop.Sdk//26.08` is freedesktop-sdk-26.08.3, with GCC 16.2.0
  and binutils 2.47 (rustc needs a linker).
- `org.freedesktop.Sdk.Extension.rust-stable//26.08` ships rustc and cargo
  1.98.0, 557 MB download.
- GNOME Platform 50 and 51 are on Flathub.

Experiment (R-008): inside `flatpak run org.freedesktop.Sdk//26.08` with
`--nofilesystem=host --unshare=network` and the extension on `PATH`,
`cargo build --release --offline` built a synthetic 20k-instruction
generated crate in 5.3 s, the binary ran, and `$HOME/Downloads` was not
visible. That ran the runtime directly, not an app.

## Options

| | Toolchain source | Pros | Cons |
|---|---|---|---|
| A | App `runtime: org.freedesktop.Sdk`, rust-stable extension at run time | Shared, maintained, no bundling | Large first install; depends on the extension being mounted for the app (to verify); Flathub stance unknown |
| B | Bundle rustc, cargo and std into `/app` at build time, copied from the extension | Exact pinned compiler, works offline, one install | Large app (hundreds of MB), we ship toolchain updates |
| C | Ask the user to install Rust on the host | Small app | Escapes the sandbox and breaks "works as a Flatpak". Rejected |

## Decision (proposed)

Prototype A first, with B as the fallback; choose with evidence. In
either case:

- The runtime's own Rust sources and their vendored dependencies are
  shipped in `/app/share/usum-recomp/` (project sources GPL-3.0-or-later,
  dependencies under GPL-compatible permissive licenses; no game content).
  The local build compiles them together with the generated crates into
  one native **executable** under `$XDG_CACHE_HOME` (inside
  `~/.var/app/<id>/`). The launcher starts it as a child process.
  Linking an executable avoids `dlopen` and the `unsafe` that loading a
  library from Rust would need.
- Builds run with `--offline --locked` and `CARGO_HOME` inside the app's
  cache. No network is needed or allowed during a build.
- The dump is opened through the file-chooser portal (document portal
  path), read only, never copied. Saves live in `$XDG_DATA_HOME`.

## What must be tested (milestone 6 exit criteria)

1. From an installed app (not `flatpak run <runtime>`): the toolchain is
   present at its expected path with the expected version.
2. An offline, vendored build succeeds with `--share=network` absent.
3. The built executable runs from `~/.var/app/<id>/cache` (no `noexec`).
4. A dump opened through the portal is readable by the build and by the
   game process, and nothing outside it is.
5. Build time and peak memory are recorded for both games on a reference
   machine and shown in the UI beforehand.
