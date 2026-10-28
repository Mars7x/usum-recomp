# ADR-0001: Our own ARM → Rust static recompiler; 3dsrecomp only as a reference

- Status: accepted
- Date: 2026-10-25

## Context

The project requires static recompilation, with all project-authored code
in Rust *and* the generated code in Rust. The only public recompiler found
that handles these games, fearkov/3dsrecomp (MIT, Rust, `9b2f217`),
generates **C** that a C compiler builds into a library. It works on
Ultra Sun: 84,547 functions found, 662 C files (831 MiB), a 346 MiB `.so`
in 717 s on 16 cores (FEASIBILITY.md). Clockwork's recompiler is
unlicensed and targets Yo-kai Watch.

Options considered:

1. Use 3dsrecomp as is: generated C. **Violates the Rust requirement.**
2. Fork 3dsrecomp and replace its C emitter with a Rust emitter. Its
   discovery and CRO analysis are reusable Rust (MIT, with attribution),
   but its decoder and codegen are fused around C idioms (macros,
   `goto`), and its IR is not separate from emission.
3. Write our own decoder, IR and Rust emitter, using 3dsrecomp's
   analysis results as a *cross-check*: same functions found, same code
   hashes.

## Decision

Option 3, staged. Milestone 3 builds a small decoder → IR → Rust emitter
for a subset of ARM/Thumb, checked against independent references on
synthetic programs. Before writing discovery (milestone 4) we re-evaluate
importing 3dsrecomp's `discover`/`cro` modules as an MIT dependency or
vendored code with attribution. That is allowed, and preferable to
re-deriving them, if their API can be used without the C emitter.

3dsrecomp, and its C output, is used only in the research sandbox outside
the repository, to measure and cross-check. Nothing it generates is
committed or shipped, and no project build depends on it.

## Consequences

- The milestone 1 numbers measured with 3dsrecomp are evidence of
  *feasibility*, not progress on our pipeline.
- Our emitter can rely on Rust semantics. Rust never contracts float ops
  into FMAs, and `wrapping_*`/`overflowing_*` make guest integer semantics
  explicit.
- Rust compile cost is the main new risk (R2); milestone 3 measures it.
