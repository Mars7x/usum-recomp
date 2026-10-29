# Next steps

Updated 2026-10-29. Ultra Sun boots, runs the opening and menus, reaches
indoor/outdoor movement and early battles, and supports recorded replay tests.
Full-game completion and Ultra Moon gameplay parity remain unverified.
See [compatibility](COMPATIBILITY.md), [building and controls](BUILDING.md),
[roadmap](ROADMAP.md) and the chronological [research log](RESEARCH.md).

## Current implementation

- ARM/Thumb/VFP translation covers the supported executables and their CRO
  modules. Generated workspaces contain deterministic `probe` and interactive
  `play` programs. Translation output remains private in the user's cache.
  Build cost depends on toolchain, job count, RAM and generated changes; early
  research timings are historical measurements, not current promises.
- The 3DS compatibility runtime supplies checked memory, threads and kernel
  objects, services, input, audio, save handling, module loading and restart.
  Software PICA rendering remains the comparison reference.
- Opt-in Vulkan acceleration executes vertex and fragment work, keeps supported
  color/depth and display data resident, and presents through the GPU with
  fallback. Internal resolution scaling is 1×–8×; 6× is 2400×1440 for the top
  screen. Scaling changes rasterization resolution, rather than only enlarging
  the window. Original low-resolution textures and 2D elements remain limited
  by their source assets.
- Keyboard, mouse touch and mapped controllers work. Controllers connected
  after launch are discovered; disconnect and focus loss release inputs.
- F3/F4 adjust resolution; F5 toggles title statistics; F6 changes the host
  presentation cap; F7 toggles the testing overlay, which is off by default.
  These controls do not unlock the game's native frame rate.
- Recent validated work includes GPU-resident window presentation (R-115),
  byte-packed script/area callback discovery (R-116), checked register-list
  memory transfers (R-117), presentation timing/HUD controls (R-118) and exited
  thread TLS reuse with retained handle identity (R-119).

## Active work

1. **Native dynamic FPS.** Keep this a core feature over the compatibility
   runtime. Identify actual engine frame completion and simulation timing before
   uncapping. Preserve movement, animation, encounters, battle/event timing and
   audio at normal speed while producing additional rendered frames. The target
   is at least 200 actual gameplay frames per wall second at 6×. It is unmet.
   No interpolation, extrapolation, repeated window presents or faster game
   clock may be counted toward that target.
2. **Profile-led optimization.** Revisit intro/movie helpers, checked memory,
   frequent guest helpers, vertex preparation and GPU synchronization using
   fresh profiles. Retain a reference path and frozen before/after executables.
   Require identical replay inputs, starting saves, captures, audio and guest
   counters before accepting a timing-neutral optimization. Benchmark without
   captures/profiling, repeat in balanced order, and wait for encoders rather
   than interrupting them. Reject overlapping measurements and changes without
   demonstrated benefit. See R-084/R-085/R-098 and later investigations.
3. **Compatibility and graphics.** Keep fixes narrowly scoped and replayable.
   The recorded thread-resource stop is reproduced and the failed SVC succeeds
   with TLS recycling in the same state; gameplay beyond it is not validated by
   that check. Continue investigating rendering differences and reported menu/
   transition problems without treating one fix as full-game compatibility.
   Outlines should remain enabled by default; a user-facing outline toggle is
   planned.
4. **Sound quality.** PCM/DSP-ADPCM source decoding, mixing and source filters
   exist; master-volume/effect behavior and hardware accuracy remain incomplete.
   A slow host can still produce audio gaps. Intro performance must be measured
   separately from gameplay.
5. **Usable distribution.** Finish the launcher, portal import, build progress,
   offline toolchain and Flatpak workflow (M6/ADR-0003), then expand host/GPU and
   Ultra Moon validation. Neither packaging nor an ARM Linux build is released.

## Measurement limits

The HUD's `FPS` line currently counts top-screen GSP framebuffer submissions
per real second; `WIN` counts successful window presents. Submissions do not
prove unique images or native engine frame completion. Unthrottled `probe`
throughput also does not establish normal-speed uncapped FPS. Keep these
observations separate when reporting progress toward 200 FPS.

## Development boundaries

Keep the 3DS runtime as the compatibility foundation. Targeted native helpers
may be evaluated where profiling and semantic validation support them; a full
native rewrite is not the objective. Do not change another contributor's active
work. Commit substantial validated progress and update public status documents
without publishing game-derived material.

Only audited project source and original synthetic tests belong in Git. ROMs,
translations, saves, recordings, screenshots, audio and raw GPU captures stay
private. Differential tests use external QEMU/ARM binutils; see BUILDING.md.
Historical hypotheses and measurements remain in RESEARCH.md rather than being
silently rewritten as current facts.
