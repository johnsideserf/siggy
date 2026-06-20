---
title: Splitting render-time state out of App (the ui::draw &mut App problem)
date: 2026-06-20
category: architecture
module: ui / app
problem_type: architecture
component: rendering
symptoms:
  - "ui module documented as stateless but draw() takes &mut App"
  - "scroll clamp, focus derivation, at_top, mouse hit-rects, and link/image caches are written during the render pass"
  - "focus derivation/clearing inside render is frame-order-dependent and was untestable without a fake terminal"
root_cause: architecture
resolution_type: partial_plus_plan
severity: medium
tags: [rendering, ratatui, borrow-checker, viewstate, refactor, issue-496]
---

## Problem

`ui::draw(frame, app: &mut App)` is documented as a stateless render layer but it
mutates `App` during the pass: it clamps `scroll.offset` to the rendered content
height, derives/clears `scroll.focused_index`/`focused_time`, sets
`scroll.at_top`/`can_extend_in_memory`, captures mouse hit-rects
(`mouse.messages_area`/`input_area`/`sidebar_inner`), and fills the render caches
(`image.link_url_map`, `image.visible_images`, kitty transmit lists, the
line-height cache). This is issue #496.

## What is already done (incremental, safe)

The *testability* half is resolved. Every derivation that was buried in the
render loop is now a pure, unit-tested function in `ui::chat_pane`:

- `window_start(total, available_height, window_extra)` - bottom-anchored first index
- `bottom_align_scroll(content_height, available_height, offset)` - `(base_scroll, clamped_offset, scroll_y)`
- `ensure_focus_visible(msg_start, msg_end, base_scroll, scroll_y, available_height)`
- `focus_line_span(line_heights, line_msg_idx, focus_idx)` - the focused message's cumulative line span

The render pass now reads inputs, calls these, and assigns the results (the
"legitimate layout feedback" a TUI must do). The real (non-stateless) `draw()`
contract is documented on the module and on `draw()`. Render-path coverage that
did not exist before now does: snapshots at `scroll.offset = 6`, a clamped
`scroll.offset = 100_000`, and a Normal-mode focused-message-scrolled-into-view.

## Why the remaining "ViewState" split is all-or-nothing

The issue's option 1 is "split a `ViewState` (scroll, mouse rects, link/image
caches) passed `&mut` while the rest of App is `&`." The borrow checker makes
this **not** incrementally sliceable:

- You cannot hold `&App` and `&mut app.scroll` at the same time: `&App` borrows
  *all* of `App`, including `scroll`. So a `ViewState` that borrows
  `&mut app.scroll` plus `&app` does not compile.
- The only ways to get disjoint access are (a) destructure `App` into per-field
  references at the top of `draw` and thread the ~20 fields `draw_messages` reads
  through every `ui/` render signature, or (b) restructure `App` so the
  render-mutated fields live in a separate top-level value (`ViewState`) owned
  *alongside* `App`, not inside it.

Option (b) is the clean end state but it is cross-layer: `mouse` hit-rects are
read next frame by `handlers::keys::handle_mouse_event`, and `image.link_regions`
is read post-render by the OSC 8 injector, so moving those off `App` means
threading the new `ViewState` through `drain_events`, `dispatch_send`,
`handle_signal_event`, the key handlers, and the main loop - i.e. the same
surface the #494 handler extraction just touched. It is one large PR, not a
series of small ones.

## Recommended plan for the dedicated pass

1. Introduce `struct ViewState { scroll: ScrollState, mouse: MouseState, image: ImageState }`
   owned by `run_app` next to `App` (remove those three fields from `App`; the
   field-count ratchet drops by 2, which passes).
2. Change `draw(frame, app: &App, view: &mut ViewState)` and thread
   `(&App, &mut ViewState)` through `draw_sidebar` / `draw_chat_area` /
   `draw_messages` / overlays. Leaf functions that only read `App` are unchanged.
3. Update the handler layer and post-render OSC 8 injector to take
   `&mut ViewState` / `&ViewState` for the rects, link regions, and caches.
4. Verify against the now-existing safety net: the four pure-function unit tests,
   the three scroll/focus snapshots, and the full insta suite (demo data renders
   are snapshot-locked).

The pure derivation already lives outside the render mutation, so this pass is
mechanical borrow plumbing rather than logic change - but it is wide, so do it as
one focused, reviewed change rather than squeezed onto an unrelated branch.

## Lesson

When a render pass legitimately needs layout feedback (scroll clamping depends on
the realised wrapped height), the high-value, low-risk move is to extract the
*derivation* into pure tested functions and add render snapshots first. That
removes the actual hazard (untestable, frame-order-dependent logic) immediately
and leaves only correctness-neutral borrow plumbing, which can then be done
safely against real coverage.
