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

## Measured constraint: the render layer reads almost all of App

A borrow-split (`&App` + `&mut ViewState`, or a `ViewCtx` of granular field
references) only pays off if the render layer reads a *small, separable* slice of
`App`. It does not. The `ui/` layer touches **66 distinct `app.*` members**
(`grep -rhoE "app\.[a-z_]+" src/ui/ | sort -u | wc -l`), spanning roughly forty
data fields (every overlay state, `store`, `theme`, `scroll`, `mouse`, `image`,
`sync`, `input`, `autocomplete`, `lock`, ...) plus helper methods.

So a `ViewCtx` borrow struct would have to enumerate ~50 fields and be threaded
through every render function - which is just "pass all of `App` field-by-field."
That is precisely why `draw` takes `&mut App`. Moving the three mutated
sub-structs *off* `App` instead is worse: it breaks every `App` method that uses
them alongside other state (`ensure_active_images`, the key handlers), forcing
those off `App` too - a cross-layer rewrite.

## Revised recommendation

Given the read-set above, the clean `ViewState` split is not worth its cost, and
the issue's *substantive* concern is already resolved:

- The hazard the issue actually names - focus derivation/clearing being
  "frame-order-dependent and untestable" - is gone: that logic is now pure,
  unit-tested functions (`window_start`, `bottom_align_scroll`,
  `ensure_focus_visible`, `focus_line_span`) plus snapshot coverage at non-default
  scroll/focus.
- The remaining render-time writes are the layout feedback a TUI must do
  (clamping scroll to the realised wrapped height) and frame caches; the `draw`
  contract is now documented honestly rather than claimed "stateless."

Recommendation: treat #496 as resolved by the derivation extraction + contract
docs + snapshots, and keep `&mut App` for `draw`. Only revisit a borrow split if
`App` is later decomposed for other reasons. The original plan below is retained
for that hypothetical.

## Original plan (retained, only if App is decomposed later)

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
