# ADR-0006: Guest threads as saved contexts on one host thread

- Status: accepted
- Date: 2026-10-25

## Context

The game creates threads (R-027). Translated code returns to the host at
every SVC and at every budget exit, and keeps no guest state on the host
stack across those points, so a `Cpu` value is a thread's complete
state. The 3DS kernel schedules by strict priority: a thread is
preempted only when a higher-priority thread wakes, and there is no time
slicing (3dbrew "Multi-threading"). Applications run on core 0; core 1
is the system's, which an application may be granted a share of (APT
SetApplicationCpuTimeLimit).

## Decision

- Guest threads are `Thread { cpu, priority, core, state }` entries in
  the kernel. The run loop runs one at a time. An SVC that blocks,
  exits, or wakes a more urgent thread returns `Outcome::Switch`, and
  the loop calls `Kernel::switch`, which saves the context and loads the
  most urgent ready thread. Among equals it rotates.
- The budget belongs to the time slice, not to a thread. At a budget
  exit the loop calls `Kernel::time_slice`. If a thread on the *other*
  core is ready, it runs: this stands in for two cores running at once,
  so that a thread spinning on another core's progress cannot hang the
  run. Within a core, priorities are strict.
- Each thread has its own TLS slot (0x200 bytes from 0x1FF82000, 32
  slots); a new thread's FPSCR is 0 (an open question, R-017).
- Waits are kernel-side state (`Wait`: handles, wait-all, deadline,
  arbiter address). Wake-ups write the result into the saved context.
  When no thread is ready and one has a deadline, the host sleeps until
  it.

## Consequences

- Deterministic: one host thread, scheduling decided only at SVCs and
  budget exits. Races that need true parallelism do not happen. A game
  that depends on two cores advancing together runs differently, and
  this would show as a hang (budget exhaustion) or a divergence, not
  silently.
- No locking in guest memory or the kernel.
- Real parallelism (core 1 on its own host thread) stays possible
  later: it needs shared memory with atomics for exclusive accesses,
  and is not needed while the game runs on one core.
