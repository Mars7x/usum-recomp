# Architecture

Status: 2026-10-29. The runtime executes translated Ultra Sun gameplay; launcher
and Flatpak integration below remain planned. See [compatibility](COMPATIBILITY.md).

## Pipeline (target)

```
user's dump ──(portal, read-only)──▶ usum-import   validate, hash-check, identify profile
                                         │  code + CROs (in memory, never written out raw)
                                         ▼
                                     usum-arm       decode ARM/Thumb/VFP → IR
                                     usum-recomp    discovery, CRO handling, IR → Rust source
                                         │  generated crates in $XDG_CACHE_HOME (never in git)
                                         ▼
                rustc/cargo (Flatpak SDK extension, --offline) builds
                generated crates + usum-runtime sources ──▶ native game executable
                                         │
launcher ──spawns──▶ game executable:  usum-runtime   GuestMemory, kernel/SVCs, HLE services
                                       usum-render    PICA200 → wgpu
                     reads RomFS from the dump at run time; saves in $XDG_DATA_HOME
```

## Workspace today

| Crate | Role | Status |
|---|---|---|
| `usum-import` | Input contract: containers, integrity, code, RomFS, CROs, profiles, synthetic fixtures | implemented, tested on both real dumps |
| `usumctl` | Development CLI over the libraries | `inspect`, `modules`, `analyze` (`--detail`, `--leaves`), `compare`, `translate`, `make-fixture`, `paths`, `doctor` |
| `usum-guest` | `Cpu`, `Memory` (permission-checked regions), the `Exit`/`Fault` protocol, shared instruction semantics (`ops`, `vfp`), the host run loop. The only interface generated code may use | everything the executables use; FPSCR exception flags not modelled (R-017) |
| `usum-arm` | ARM and Thumb (ARMv6K + VFPv2) decoding into `Instr`; PC-relative values resolved at decode time | every real instruction control flow reaches in both executables (R-019) |
| `usum-recomp` | Discovery (recursive descent, calls, jump tables, pointer seeds), executable/CRO translation and Rust emission | translates supported executables and modules; missing runtime targets still fail loudly |
| `usum-runtime` | Checked guest memory integration, kernel/SVCs, services, software PICA, audio, input, saves, deterministic `probe` and interactive `play` | Ultra Sun reaches gameplay and early battles; compatibility remains incomplete |
| `usum-render` | Vulkan rasterization, vertex processing, scaled targets and window presentation; default in generated `play` builds | persistent color/depth targets, textures/TEV/lighting, immutable deferred readbacks and GPU-resident display frames; guarded runtime fallback and explicit software reference retained |
| `usum-difftest` | Test-only: synthetic programs, random-instruction corpora and (game mode) real game functions, run under QEMU and as recompiled Rust, compared byte for byte | 22 programs + random corpora pass; 4 game functions verified |

Planned, created only when their milestone starts (no empty crates):

| Crate | Milestone | Boundary justification |
|---|---|---|
| `usum-launcher` | M6 | UI, import flow, build orchestration |

## Generated code

One Rust function per guest function, named `f_XXXXXXXX` (ARM) or
`t_XXXXXXXX` (Thumb), entered at any of its labels:

```rust
pub fn translated_example(cpu: &mut Cpu, mem: &mut Memory) -> Result<Exit, Fault> {
    let mut pc = cpu.r[15];
    loop { match pc {
        0x00100104 => { /* budget check; one block of instructions */ }
        ...
        _ => { cpu.r[15] = pc; return Ok(Exit::Jump); }   // somewhere else: the caller or host continues
    } }
}
```

- Direct calls (BL/BLX imm) are Rust calls, limited to `DEPTH_LIMIT` (256)
  nested frames. Beyond that the code unwinds to the host, which resumes
  from `r[15]`. Every return point is a label, so this is safe.
- SVCs return `Exit::Svc` to the host, which resumes at the following
  instruction (also a label).
- Indirect jumps continue locally if the target is one of this
  function's labels in the same state, otherwise `Exit::Jump`.
- The host finds code by a sorted `ENTRIES` table keyed by address | Thumb
  bit. No entry means `Fault::NoTranslation`: there is no interpreter
  to fall back on.
- Every block subtracts its instruction count from `cpu.budget` and
  returns `Exit::Budget` when it runs out, so a hang is a reported failure,
  not a stuck test. The runtime uses these exits for scheduling guest threads.

## Timing and presentation

The compatibility runtime retains the guest instruction budget, virtual clock,
VBlank scheduling and service behavior. `play` throttles that execution toward
normal wall time; an overloaded host can still run behind. Frame images and
their submission versions travel together through a coalescing mailbox to the
window thread. Presentation caps limit successful window updates independently
of guest simulation, input polling and audio.

The testing HUD distinguishes top-screen framebuffer submissions from host
presents. Neither counter proves unique engine frame completion. Native dynamic
FPS must produce additional rendered frames while preserving simulation and
audio timing; that work remains unfinished. No frame interpolation or generated
frames are implemented. See [R-118](RESEARCH.md#r-118-2026-10-29--core-presentation-policy-timing-measurements-and-testing-hud).

## Rules

- **Safe Rust.** `unsafe_code = "forbid"` is set workspace-wide. Should a
  crate ever need `unsafe` (for example a memory-mapping fast path), it
  lifts the lint for that one module only, documents the invariants
  there, and adds tests aimed at them.
- **Guest semantics are explicit.** Guest integers are `u32` with
  `wrapping_*`/`overflowing_*`; Rust's overflow behaviour is never relied
  on. Guest addresses are `u32` values, never host pointers. Memory access
  goes through `GuestMemory`, with bounds and permission checks.
- **Untrusted input.** Everything read from a dump is bounds-checked and
  size-capped (`usum_import::source`).
- **No interpreter in the execution path.** A missing translation is a
  reported error that feeds discovery hints, not a fallback.
- **Two games, one pipeline.** Profiles carry all per-version facts;
  identical modules (by hash) share translated output.
