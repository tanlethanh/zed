---
title: "Zed Development: GPUI Conventions"
description: "Core GPUI concepts and conventions used in Zed."
---

# Zed Development: GPUI Conventions

This page documents the GPUI patterns used across Zed.

It focuses on lifecycle behavior, state flow, and conventions that matter
when you are implementing features.

## Mental model

GPUI combines:

- app-owned state (`App` + `Entity<T>`)
- declarative view rendering (`Render`)
- imperative element layout/painting (`Element`)

Most feature work lives in entities and views. You only drop to custom elements
when you need low-level control.

## Core types and contexts

### App and Context

- `App` is the root state owner and service entry point.
- `Context<T>` is an entity-scoped wrapper around `App` with entity-specific
  APIs like `notify`, `emit`, `observe`, and `subscribe`.
- `Context<T>` dereferences to `App`, so APIs that take `&App` generally also
  work with `&Context<T>`.

### Window

- `Window` manages per-window UI state (focus, drawing, input dispatch).
- It is not a context. Use `App`/`Context<T>` for entity operations.
- When both are present, method signatures typically take `window` before `cx`.

### Entity and WeakEntity

- `Entity<T>` is a typed handle into state owned by `App`.
- `WeakEntity<T>` is a weak handle for async/lifecycle-safe references.
- Use `Entity::read`, `read_with`, `update`, and `update_in` to access state.

> **Warn:** Updating an entity while it is already being updated can panic.

## Rerender cycle

The rerender model is frame-driven.

1. GPUI calls `Render::render()` on the root view.
2. The view builds an element tree for the current app state.
3. Elements run the pipeline:
   - `request_layout`
   - `prepaint`
   - `paint`
4. Before the next frame, the element tree and callbacks are dropped, then
   rebuilt from state.

State invalidation is explicit:

- call `cx.notify()` when a view-relevant state change occurs
- `notify` triggers observer callbacks (`observe`) and schedules rerender work
  through GPUI's effect queue

## Frame invalidation (`refresh` vs `notify`)

GPUI separates **scheduling a frame** from **marking content stale**.

### Platform frame loop

Each platform calls the window's `on_request_frame` callback (display link on macOS,
compositor callbacks on Linux, and similar drivers on mobile). Inside that callback,
GPUI draws only when the window invalidator is dirty or `RequestFrameOptions::force_render`
is set. Calling `request_frame` every vsync does not repaint by itself.

### `window.refresh()`

`window.refresh()` sets the window invalidator dirty and sets `window.refreshing`,
which bypasses cached view reuse for the next draw. Use it when ephemeral UI state
changes during input handling and you need a full-window redraw soon—for example
pending click state, active/pressed styling on elements, or drag feedback handled
in `Window::dispatch_mouse_event`.

Do not call `refresh()` on every mouse or pointer move by default. That defeats view
caching and was reduced upstream in
[gpui: Reduce `window.refresh` (#25009)](https://github.com/zed-industries/zed/pull/25009).

### `cx.notify(entity)` (preferred for view-owned state)

When hover, scroll offset, or other state lives under the current view, invalidate
that view instead of refreshing the whole window:

```rust
let current_view = window.current_view();
// after updating hover_state or scroll_offset:
cx.notify(current_view);
```

`notify` marks the entity dirty via `WindowInvalidator::invalidate_view` and is the
usual pattern in `div` for hover and scroll listeners.

### Conventions in `div` (upstream)

| Input / state change                   | Invalidation              |
| -------------------------------------- | ------------------------- |
| Hover / group hover on move            | `cx.notify(current_view)` |
| Scroll offset change                   | `cx.notify(current_view)` |
| Active / pressed styling on down or up | `window.refresh()`        |
| Pending click / drag start or end      | `window.refresh()`        |

GPUI does **not** call `refresh()` inside every `on_mouse_event` handler at the
framework level. `Window::dispatch_event` only refreshes on input modality changes;
drag move/end still refresh from `dispatch_mouse_event`.

### Mobile and platform-specific code

Follow the same invalidation rules on all targets:

- Do not gate `window.refresh()` with `#[cfg(not(target_os = "android"))]` (or similar)
  in shared element code. Upstream keeps `refresh()` for active and click paths on
  every platform.
- If a platform runs a continuous frame callback (for example Choreographer), you
  still must mark the window dirty when UI state changes; otherwise frames only
  re-present the last scene.
- Prefer `cx.notify(current_view)` over blanket `refresh()` when the change is
  scoped to the current view. Coalesce extra dirty marks in the input pipeline if
  needed, rather than skipping invalidation on one platform.

## Focus cycle

Focus is represented with `FocusHandle` and managed per window.

Key operations:

- `window.focus(&handle, cx)`
- `window.focus_next(cx)` and `window.focus_prev(cx)` for tab-stop navigation
- `window.blur()` and `window.disable_focus()`
- `window.focused(cx)` to read current focus

Important behavior:

- focus changes refresh the window
- focus notifications are deferred with `cx.defer(...)` to avoid re-entrant
  entity updates in the same effect cycle

## Actions and key dispatch

Actions encode logical keyboard operations (not raw key events).

Define actions with:

- `actions!(namespace, [ActionA, ActionB])` for unit actions
- `#[gpui::action]` for richer action payload types

Bind and handle with:

- `.key_context("context_name")`
- `.on_action(...)` handlers on elements

Dispatch flow uses capture and bubble phases along the dispatch tree, with
focused context driving key handling precedence.

## Component and action snippets

This section is a copy-paste starting point for the most common GPUI pattern:
create a view component, handle an action, and dispatch that action.

### Create a component (view entity)

```rust
use gpui::{
    actions, div, App, Context, Entity, IntoElement, ParentElement, Render, Window,
};

actions!(counter, [Increment]);

pub struct Counter {
    value: usize,
}

impl Counter {
    pub fn new(_window: &mut Window, _cx: &mut App) -> Self {
        Self { value: 0 }
    }
}

impl Render for Counter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("counter")
            .on_action(cx.listener(
                |this: &mut Counter, _action: &Increment, _window, cx| {
                    this.value += 1;
                    cx.notify();
                },
            ))
            .child(format!("Count: {}", self.value))
    }
}

fn create_counter(window: &mut Window, cx: &mut App) -> Entity<Counter> {
    cx.new(|cx| Counter::new(window, cx))
}
```

### Dispatch an action

```rust
use gpui::{actions, Window};

actions!(counter, [Increment]);

fn trigger_increment(window: &mut Window, cx: &mut gpui::App) {
    window.dispatch_action(Increment.boxed_clone(), cx);
}
```

You can also dispatch via a `FocusHandle` when you want to target a specific
focused subtree.

### Keymap binding snippet

```json [keymap]
{
  "context": "counter",
  "bindings": {
    "ctrl-k": "counter::Increment"
  }
}
```

## Events, observing, and subscriptions

GPUI uses two different channels:

- `notify`/`observe` for "state changed"
- `emit`/`subscribe` for typed events

Use `notify`/`observe` when consumers should re-read entity state.

Use `emit`/`subscribe` when payload data should be delivered directly.

`Subscription` lifecycle rules:

- dropping a `Subscription` unsubscribes
- `detach()` keeps the callback active without storing the handle
- be intentional with detached subscriptions to avoid long-lived behavior

## Foreground and background tasks

All rendering/entity access happens on GPUI's foreground thread.

Task APIs:

- `cx.spawn(...)` runs async work on the foreground executor
- `cx.background_spawn(...)` runs async work on the background executor

Task lifetime rules:

- dropping a `Task<R>` cancels it
- keep tasks alive by awaiting them, storing them, or detaching
- use `Task::ready(value)` for immediate futures

Async contexts:

- `to_async` creates `AsyncApp` / `AsyncWindowContext`
- async entity/window access is fallible because app/window may be gone

## Effect cycle and lifecycle timing

GPUI queues side effects and flushes them after updates.

The flush cycle processes effects such as:

- `Notify`
- `Emit`
- `RefreshWindows`
- `Defer`

During flush, GPUI also:

- releases dropped entities (`release_dropped_entities`)
- releases dropped focus handles (`release_dropped_focus_handles`)

This design avoids deep re-entrant chains and centralizes ordering of observer,
event, and deferred behavior.

## Common conventions in Zed

- Prefer entity-driven state and `Render` views over custom `Element`
  implementations.
- Use the inner closure `cx` provided by `update`/`update_in`, not an outer
  `cx`, to avoid borrow issues.
- Propagate fallible operations with `?`; avoid `unwrap()`.
- Do not silently ignore fallible results with `let _ = ...`.
- Use full variable names.
- Keep comments sparse and explain why, not what.

## Testing conventions

- Prefer `#[gpui::test]` and GPUI test contexts for deterministic behavior.
- When driving scheduler-based tests, prefer GPUI executor timers:
  `cx.background_executor().timer(duration).await`
- Avoid `smol::Timer::after(...)` in tests that rely on
  `run_until_parked()`.

## Practical checklist

When adding a GPUI feature in Zed:

1. Put long-lived state in an `Entity<T>`.
2. Update state via `update`/`update_in`.
3. Call `cx.notify()` for render-relevant changes.
4. Use actions + key context for keyboard behavior.
5. Use `emit` events only when typed payload delivery is needed.
6. Prefer background tasks for heavy work, foreground tasks for UI-bound
   coordination.
7. Ensure task/subscription lifetimes are explicit.
