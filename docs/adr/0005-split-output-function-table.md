# ADR-0005: Generated code in many crates, calls across crates through a function table

- Status: accepted
- Date: 2026-10-25

## Context

One crate of ~350k translated instructions exhausted 31 GB of RAM in
rustc (R-013). Ultra Sun's executable alone translates to ~1.76M
instructions (R-022). Generated code must be split into compilation
units of bounded size that build in parallel. Calls between translated
functions are cyclic (A calls B, B calls A), but Rust crates cannot
depend on each other cyclically.

## Decision

- `usum_recomp::emit::split` packs functions, in address order, into
  units of about N instructions (default 20,000: about 0.9 GB of peak
  memory per unit in release builds).
- A call within a unit is a direct Rust call. A call across units goes
  through `usum_guest::function(index)`, a process-wide table of `Code`
  pointers in a `OnceLock`. The top crate, which depends on every unit,
  defines `FUNCTIONS` (index order) and the sorted `ENTRIES`, and installs
  the table once at startup with `usum_guest::install_functions`.
- Everything is safe Rust: no `extern` declarations, no `#[no_mangle]`,
  no linker tricks.
- `usumctl translate` writes one Cargo workspace per profile under
  `$XDG_CACHE_HOME/usum-recomp/translations/<profile>/` and refuses any
  directory inside a git work tree.

## Consequences

- A cross-unit call costs one atomic load and an indirect call.
- One process holds one table, so one game per process (true for the
  product). The differential tests translate exactly one program in split
  mode.
- Every translated function dispatches on `pc`, so a wrong table entry
  would still execute correctly via the host (a slow path), and output
  comparison cannot see it. `usum_guest::call` therefore has a debug
  assertion that the called code ran at least one block. A deliberately
  wrong index was caught by it.
