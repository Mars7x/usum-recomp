# ADR-0002: Runtime in Rust, HLE of kernel and services; reuse decided per subsystem

- Status: proposed (decided per subsystem at milestone 5)
- Date: 2026-10-25

## Context

Recompiled code needs guest memory, SVCs, ~34 permitted services, CRO
loading, the GPU, DSP and input (FEASIBILITY.md §1, §4). Existing work:

| Source | Language | License | Usable? |
|---|---|---|---|
| fearkov/zakuro `951e978` | Rust, ~67k lines (core 25k, GPU 27k) | MIT | Legally yes. It is an emulator *application*: services are tied to its interpreter-based CPU and its own frontend |
| Clockwork | C++ | none | No |
| Azahar/Citra | C++ | GPLv2+ | No (C++; and GPL would bind the project) |

## Decision (proposed)

- The runtime is our own Rust crate set, built around *our* generated
  code's interface: a `GuestContext`, `GuestMemory` and an SVC dispatcher.
  There is no interpreter in the execution path.
- For each HLE subsystem (fs, cfg, apt, hid, gsp/GPU, dsp, ...) we choose,
  with evidence, between writing it ourselves and adapting Zakuro's MIT
  Rust code, with attribution and a recorded upstream revision.
  Adapting is preferred when the code is separable and readable.
  "Adapting" means pulling service logic into our crates, never running
  Zakuro's CPU core.
- 3dbrew is the primary behavioural reference. Citra/Azahar source may be
  read to understand behaviour, but no GPL code is copied.

## Consequences

The runtime is the largest workstream. Each subsystem's ADR records which
path it took and why.
