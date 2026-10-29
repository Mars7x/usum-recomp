# Feasibility report (milestone 1)

Date: 2026-10-25. Every claim is tagged **[verified]** (measured here, with
the command in [RESEARCH.md](RESEARCH.md)), **[source]** (documented by a
cited primary source, not re-measured), **[claim]** (reported by a third
party, not verified) or **[hypothesis]/[unknown]**.

This is the historical milestone-1 assessment. Its statements about what runs
describe that date. As of 2026-10-29, Ultra Sun reaches gameplay and early
battles; see [current compatibility](COMPATIBILITY.md), [architecture](ARCHITECTURE.md)
and the [research log](RESEARCH.md) for subsequent implementation evidence.

## Verdict

Feasible in principle, and nothing found so far rules out the intended
approach. The input side is solved and verified. The translation side
has a credible precedent: a public MIT recompiler already translates these
exact executables to C that compiles **[verified]**. Our requirement that
the generated code be Rust raises two risks that only experiments can
settle: compile cost and runtime performance. The biggest body of work is
the runtime, especially the GPU and DSP, not the CPU translation. Neither
game runs in this project today.

## 1. What "native recompilation" means here

The game's ARM11 code (ARMv6K: ARM, Thumb, VFPv2) is translated ahead of
time into Rust source. A Rust compiler on the user's machine then turns it
into native x86-64 code, linked with our Rust runtime. Nothing interprets
or JIT-compiles game instructions in a release build.

What still needs compatibility implementations (HLE, in Rust):

- **Guest memory model.** Game data holds 32-bit pointers, so a 32-bit
  guest address space with 3DS mapping semantics stays. This is a data
  layout, not emulation.
- **Kernel (SVCs).** Threads, scheduling, mutexes, semaphores, events,
  timers, address arbiters, shared memory, IPC sessions.
- **Services.** The exheader permits these 34 **[verified]**:
  `$hioFIO $hostio0 $hostio1 cfg:u fs:USER gsp::Gpu hid:USER ndm:u pxi:dev
  APT:A ac:u act:u am:app boss:U cam:u cecd:u dlp:FKCL dlp:SRVR dsp::DSP
  frd:u http:C ir:USER ldr:ro mic:u news:u nfc:u nim:aoc nwm::UDS ptm:u
  qtm:u soc:U ssl:C y2r:u`. Permitted is not the same as used. Which ones
  the games actually call at boot and in play is **[unknown]** until the
  runtime logs it. Many, such as `http:C`, `soc:U`, `nfc:u` and the
  `$hostio` debug ports, likely need only "unavailable" answers
  **[hypothesis]**.
- **Hardware behind services.** The PICA200 GPU via `gsp::Gpu` command
  lists, the DSP via `dsp::DSP`, and the Y2R and camera blocks.
- **CRO loading.** `ldr:ro` maps and relocates the 132 CRO modules at run
  time **[verified: present, service permitted]**.

## 2. Static recompilation vs. alternatives

| | Static recompilation (intended) | Dynamic translation (JIT/interpreter) | Manual Rust reimplementation |
|---|---|---|---|
| Fidelity | Exact, if every reachable instruction is found and translated correctly | Exact, handles any code it meets | Only as good as the reverse engineering |
| Coverage risk | Code never found statically has no translation | None | Total: every system rewritten |
| Performance | Best: the host compiler sees whole functions | JIT good, interpreter poor | Best |
| Effort | Recompiler + runtime | Runtime + JIT (an emulator) | Enormous: ~84k functions **[verified count, 3dsrecomp]** |
| Fits project goal | Yes | No: this is an emulator | Yes, but not "from the user's dump" |

**Direction: static recompilation.** Proposed deviations, both explicit
and both kept out of release builds:

1. **Reference interpreter, test-only.** A small Rust ARM interpreter
   may serve as one of the test oracles in milestone 3. It never runs in
   a shipped build. Release builds treat "no translation for this
   address" as a fatal, reported error (see risk R1).
2. **Hand-written replacements**, used only where translation cannot
   express a behaviour (for example a hardware-timing loop). Each one is
   documented and tied to an address and a profile.

## 3. CPU-side challenges

| Topic | What we know | Plan |
|---|---|---|
| Instruction semantics | ARMv6K ARM/Thumb plus VFPv2, documented in the ARM ARM (DDI 0406 / ARMv6 DDI 0100) **[source]** | Decoder + small IR + Rust emitter; differential tests against an independent reference (milestone 3) |
| Executable discovery | 3dsrecomp's analysis reached 93.2 % of code bytes, classified 4.2 % as literal pools, and left 2.6 % unreached, over 84,547 functions **[verified]** | Same signal sources (calls, pointers in data, relocations, CRO exports and imports), plus a hint file produced by our own runtime when it meets an untranslated address |
| Indirect control flow | 41,941 indirect branches and 2,664 jump tables **[verified]** | Jump-table recovery; a sorted entry table for computed targets; returns are dispatched, not assumed |
| Memory layout | text `0x00100000` (1210 pages), rodata `0x005BA000` (173), data `0x00667000` (61), bss `0x301BC`, stack `0x40000`, identical in both games **[verified]** | 32-bit guest addresses in an explicit `GuestMemory` with region permissions; all arithmetic wrapping or checked by design |
| Relocations / CROs | CROs load at addresses chosen at run time, and the loader patches relocations **[source: 3dbrew CRO0]** | Translate each CRO position-independently (offsets plus a per-module base, as 3dsrecomp does); apply relocations to data at load |
| Floating point | VFPv2 has flush-to-zero and default-NaN modes in FPSCR; games commonly run in RunFast mode **[source: ARM ARM]**, but what USUM sets is **[unknown]** | Rust never contracts `a*b+c` into an FMA, so no flag is needed (C needs `-ffp-contract=off`, which 3dsrecomp passes). FTZ and DN are emulated explicitly in software; we do not change host FP state, since that would need `unsafe` and LLVM assumes the default |
| Threading and sync | The 3DS application core schedules by priority on one core **[source: 3dbrew Kernel]** | Guest threads become host threads serialized by a single scheduler lock, switching at SVCs and budget points. Deterministic, matches single-core semantics. Performance **[unknown]** |
| Self-modifying or generated code | None observed. CRO loading is the only code mapping we know of **[hypothesis]** | The runtime verifies loaded code against the hashes it was translated from (3dsrecomp's "stale" idea) and fails loudly on a mismatch |

## 4. Rest of the system

| Area | Approach | Main difficulty |
|---|---|---|
| Graphics | Decode PICA200 command lists from `gsp::Gpu`; translate vertex shaders (PICA ISA) to WGSL via `naga` IR; generate fragment shaders for the fixed-function combiners (TEV), fog and lighting; draw with `wgpu` (Vulkan on Linux) | float24 precision, depth and stencil formats, framebuffer reinterpretation, texture swizzling, render-to-texture. Zakuro's 26k-line GPU crate shows the scale **[verified: size]** |
| Shaders | Translated when the game uploads them, cached by hash, compiled through `naga` and `wgpu` to SPIR-V for the driver | Program-counter-dependent control flow in PICA shaders |
| Audio | HLE of the DSP's shared-memory interface (voices, ADPCM/PCM, mixing, effects) in Rust; output through a Rust audio crate | Which codecs and effects USUM uses **[unknown]**; timing |
| Input | Keyboard, gamepad (circle pad, buttons), mouse as touchscreen; mapped to `hid:USER` shared memory | Little. Needs a Rust gamepad crate; native dependencies listed in DEPENDENCIES.md |
| Dual screens | Two framebuffers (400×240 top, 320×240 bottom) composed in one window, layouts configurable | Touch mapping under scaled layouts |
| Filesystem | `fs:USER` RomFS archive served straight from the user's dump (read-only, offsets from our parser); save archives in app data | Archive semantics and error codes |
| Saves | Save data size `0xFE000` **[verified, exheader]**; stored per game and profile under XDG data, never next to the dump | Format of the save archive, and import/export of real 3DS saves |
| Other services | `cfg:u` (language/region), `ptm:u` (battery/clock), `ac:u`/`frd:u`/`boss:U`/`nim:aoc`/`news:u` (offline answers), `nwm::UDS`/`dlp` (local wireless: out of scope at first) | Getting "unavailable" answers right so the game does not hang |

## 5. Can Ultra Sun and Ultra Moon share the pipeline?

Measured **[verified]**:

- Same exheader layout: identical rodata, data, bss and stack, and text
  at the same address; text size differs by 8 bytes (`0x4B99F0` vs
  `0x4B99F8`).
- Same 34 services and 33 system-module dependencies; same save size.
- Same RomFS structure: 747 files and 133 code modules (132 CROs plus
  `static.crs`). 123 of the 132 CROs are byte-identical across the games.
  The 9 that differ have the same sizes: FieldMenu, FieldRo, KisekaeData,
  Live, MovieLib, SequencePlayer, Title, TrainerPass, Zukan.
- The executables differ (distinct SHA-256, same 5,914,624-byte size), and
  so does `static.crs`.

Conclusion: one translator, one runtime, and one set of hooks keyed by
profile. Translated output is shareable per module by content hash (123
modules), while executables and the 9 modules get per-game output.
Still needed: function-level diffs of the executables (are the 8 bytes
a shift, so most functions are identical at different addresses?) and the
runtime behaviour of both.

## 6. Largest unresolved risks

| # | Risk | Evidence so far | How we settle it |
|---|---|---|---|
| R1 | Incomplete static discovery. Without an interpreter, any missed reachable code is a crash | 2.6 % unreached, 237 statically interpreted sites in 3dsrecomp's output, mostly data decoded as code **[verified count; classification is hypothesis]**. Our discovery missed table-reached code until pointer seeds were added (R-011) | Measure in M4 against 3dsrecomp's function counts; in M5 log misses at run time and feed them back as hints. The release gate is zero misses over a playthrough corpus |
| R2 | rustc cost for millions of generated lines | **Measured (R-022):** the whole Ultra Sun executable (1.76M translated instructions, 90 crates) builds in release in 5 min 49 s on 16 cores (12 jobs), 3,519 CPU-seconds, at most 2.7 GB per rustc, about 11.7 GB in total at 12 jobs. A 4-core, 8 GB machine should take about 15 minutes at 4 jobs (extrapolated) | Mostly settled. Remaining: the CRO modules, and reducing duplicated emission of shared code |
| R3 | Performance of safe guest memory (bounds and permission checks on every access) | None yet | Milestone 3 benchmark; fast paths for direct-mapped regions using safe slices |
| R4 | GPU accuracy | Size of existing implementations | Per-feature reference tests with synthetic command lists; game frames compared only once boot works |
| R5 | DSP HLE requirements | Unknown | Log `dsp::DSP` usage at milestone 5 |
| R6 | A Rust toolchain at run time inside Flatpak | rustc 1.98.0 from `org.freedesktop.Sdk.Extension.rust-stable//26.08` built and ran a synthetic generated crate offline inside a sandboxed `org.freedesktop.Sdk//26.08` with no home access **[verified, running the runtime directly]** | Milestone 6: the same from a real app manifest (ADR-0003) |
| R7 | Flathub policy for an app that needs the SDK at run time | Not checked | Read current Flathub requirements before submitting; never claim acceptance |

## Prior art consulted

- **fearkov/3dsrecomp** `9b2f217` (MIT): ARM/Thumb/VFP → C, CRO-aware, with
  a documented host ABI. Used here as a **measurement instrument and
  cross-check only**, since our generated code must be Rust. Its analysis
  and C build of Ultra Sun succeeded **[verified]**, and its verifier
  reported 0 mismatches over a 2,000-function sample, but its reference is
  Zakuro's interpreter, so the two are not independent **[verified run]**.
- **fearkov/zakuro** `951e978` (MIT): a Rust HLE 3DS emulator that runs
  3dsrecomp output with an interpreter fallback. It lists USUM USA as
  "Playable" **[claim]**. A possible source of Rust HLE knowledge or code
  for the runtime (ADR-0002). It is an emulator, so it is never our
  execution model.
- **clockwork-recompdev/clockwork** `fa61bc2`: a static-recompilation port
  framework for Yo-kai Watch with a C++ runtime. **No license file**: all
  rights reserved by default, so nothing can be reused. Read for
  architecture ideas only.
- **N64Recomp** (github.com/N64Recomp/N64Recomp): the static-recompilation
  model for the N64; MIPS, not ARM.
- A search turned up a "P3P3DS" PSP→3DS recompiler forum post, which is
  unverified and irrelevant (wrong direction).
