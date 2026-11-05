# Single-screen play

Status: planned, nothing implemented yet (2026-11-05).

Today `play` draws the two 3DS screens in one fixed layout: the top screen
(400×240) above the bottom screen (320×240), centred and scaled together.
The left mouse button over the bottom screen touches it. This works on a
desktop monitor with a mouse, but wastes most of a wide display and needs a
pointer for anything the game only accepts by touch.

The aim is a single-screen experience: the top screen fills the display, the
bottom screen appears only when it is useful, and the whole game can be
played with a controller on a TV or handheld PC.

## Goals

- The top screen can fill the window at its own aspect ratio (5:3), with
  letterboxing on 16:9 and 16:10 displays.
- The bottom screen is always reachable in one input, and never required to
  be on screen when the game does not need it.
- Every touch interaction the game requires can be done with a controller.
- Layouts change presentation only. Game output, input timing and replays
  are identical in every layout; recordings made in one layout replay in any.

## Layouts

A layout is chosen at launch and can be cycled with a hotkey while playing.

| Layout | Top screen | Bottom screen | Intended for |
|---|---|---|---|
| Stacked | above | below | current behaviour, desktop with a mouse |
| Top focus | fills the window | picture-in-picture in a corner, can be hidden | TV, handheld PC |
| Bottom focus | picture-in-picture | fills the window | touch-heavy screens |
| Side by side | left | right | very wide displays |

Picture-in-picture size (for example ⅓ or ½ of the window height), corner
and opacity are settings. A held button can show the bottom screen at full
size and return to the previous layout on release.

## Touch without a pointer

- **Controller pointer:** a button shows the bottom screen (if hidden) and a
  cursor on it. The right stick moves the cursor and a face or shoulder
  button presses it, so drags and holds work as with a stylus. The cursor
  hides again after a short idle time.
- **Mouse and touchscreen:** touches map to the bottom screen wherever it is
  drawn, including picture-in-picture. On a touchscreen display, a tap on
  the bottom screen's area touches it directly.
- Keyboard and controller button mappings are unchanged.

## When the bottom screen is needed

Much of Ultra Sun and Ultra Moon's lower screen (the Rotom Dex map, the
menu opened with X, battle commands) can also be driven with buttons, while
some screens may only respond to touch. Which ones need touch has not been
established. Before any automatic switching:

1. Audit menus, battles, the Pokédex and boxes, Poké Pelago, Pokémon Refresh,
   Festival Plaza, Roto Loto and the QR scanner with recorded sessions, noting
   for each whether buttons alone complete it. The result goes in
   COMPATIBILITY.md.
2. Only then add an optional "show the bottom screen automatically" rule,
   based on signals the runtime can observe without game-specific memory
   reads where possible (which CRO modules are loaded, how often the bottom
   framebuffer changes), validated with the same recordings.

Manual control (hotkey, held button) stays available whatever the rule does.

## Implementation steps

1. **Layout model.** Move the fixed canvas in `play.rs` into a layout type
   that maps each screen to a rectangle and window coordinates back to
   screen coordinates. Unit tests cover every layout and window shape,
   including touches on picture-in-picture and outside both screens.
2. **Presentation.** Draw each screen as its own quad in GPU presentation
   (and the software fallback), so picture-in-picture costs no extra copies
   and the bottom screen scales with internal resolution too.
3. **Controls.** Layout cycling, the held full-size button and the controller
   pointer, configurable through environment variables first
   (`USUM_LAYOUT`, `USUM_PIP`), later through the launcher.
4. **Touch audit** as above, recorded in COMPATIBILITY.md.
5. **Optional automatic switching**, only for screens the audit shows need
   it.
6. **Launcher integration** with saved per-user settings and a handheld
   preset (for example 1280×800, top focus, controller pointer).

## Validation

- Layout mapping tests for every layout and window size.
- Replay captures are byte-identical across layouts, since layouts only change
  presentation.
- Each audited screen is completed with a controller in top-focus layout,
  using the pointer where touch is required.
