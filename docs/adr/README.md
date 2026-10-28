# Architecture decision records

| ADR | Title | Status |
|---|---|---|
| [0001](0001-recompiler-toolchain.md) | Our own ARM → Rust static recompiler; 3dsrecomp only as a reference | accepted |
| [0002](0002-runtime-strategy.md) | Runtime in Rust, HLE of kernel and services; reuse decided per subsystem | proposed |
| [0003](0003-flatpak-local-compilation.md) | Compiling the user's translated code inside the Flatpak | proposed |
| [0004](0004-game-data-policy.md) | Game data never enters the repository or a release | accepted |
| [0005](0005-split-output-function-table.md) | Generated code in many crates, calls across crates through a function table | accepted |
| [0006](0006-guest-threads.md) | Guest threads as saved contexts on one host thread | accepted |
| [0007](0007-cro-modules.md) | CRO modules translated position-independently, ahead of time | accepted |

New ADRs copy the shape (Context, Decision, Consequences), are numbered
in sequence, and cite the evidence they rest on. Superseded ADRs stay,
marked "superseded by NNNN".
