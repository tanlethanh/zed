# Native Text Interaction

GPUI has two native text interaction paths that share some platform plumbing
but represent different framework concepts:

- text selection, for read-only selectable documents in the element tree,
- input handlers, for focused text input, IME, mutation, and keyboard sessions.

The iOS backend exposes both through the window's `GPUIMetalView` as the single
`UITextInput` surface. There is no hidden composition view and no transient
selection host view.

## Text Selection

Text selection is a read-only document capability layered on top of rendered
elements. It does not own focus, text mutation, IME, or soft-keyboard policy.

Use `selection_area(...)` or `div().selection_area()` to create a selectable
document boundary:

```rust
selection_area(
    div()
        .child(StyledText::new("hello").selectable().selection_separator_after(" "))
        .child(StyledText::new("world").selectable()),
)
```

`div().selection_container()` is an alias for `selection_area()`.

`StyledText` fragments opt in with:

- `.selectable()` to register the text with the nearest selection area,
- `.selection_order(order)` to override paint-order document ordering,
- `.selection_separator_after(text)` to append copied text across fragment
  boundaries, such as a space or newline.

During layout, prepaint, and paint, `SelectionAreaElement` pushes the current
selection area onto the window stack. During paint it registers the area
identity and each selectable text fragment painted inside the area. Cached
subtree reuse must replay both area and fragment registrations.

After drawing roots, the window installs `PlatformWindow::set_selection_handler`
if any selection fragments were registered. If the frame has no fragments, GPUI
clears the selection handler and resets read-only selection state.

`WindowSelectionHandler` builds read-only `SelectionDocument`s from the last
rendered frame:

- fragments are grouped by selection-area identity,
- global element IDs are used when available,
- otherwise identity falls back to the owning view plus the area element ID,
- fragments are ordered by explicit `selection_order`, then paint order,
- ranges are stored as UTF-16 offsets because native text APIs use UTF-16.

The document answers selected range, text for a range, hit testing, first range
bounds, per-line range rects, and selection-range updates from native handles.
Replacement, marked text, and mutation callbacks are no-ops in this handler.

On iOS, read-only selection uses `TEXT_INTERACTION_NONEDITABLE`. The
Objective-C `UITextInput` methods dispatch to the selection handler while this
mode is active. `copy:` copies the GPUI read-only selection; other edit-menu
actions delegate back to UIKit.

Touch-selection methods such as `setSelectedTextRange:`,
`firstRectForRange:`, `selectionRectsForRange:`, `closestPositionToPoint:`, and
`characterRangeAtPoint:` are answered only while read-only selection is active.
This prevents focused input handlers and manual-focus surfaces from receiving
stray native carets or native selection menus.

Selection geometry is refreshed after each completed frame while noneditable
interaction is active. If the selected range, bounds, or rects changed, iOS
sends text and selection change notifications so native handles and menus can
update.

Current limits:

- built-in fragment registration is currently implemented for `StyledText`,
- selection-scoped custom actions are presented by the iOS edit menu when a
  read-only `SelectionArea` has an active selection,
- other platforms can implement native or custom presenters through the
  selection-handler slot without changing the GPUI document model.

## Input Handler

Input handlers are for focused content that receives text input. They own the
editable text contract: selected range, marked text, insertion, deletion,
replacement, bounds for caret positioning, IME preferences, and soft-keyboard
policy.

During paint, a focused element calls `Window::handle_input(...)`. GPUI queries
whether the handler accepts text input and whether the focused element used
`.manual_focus()`, then installs it with `PlatformWindow::set_input_handler`.
If the next frame has no input handler, GPUI calls
`PlatformWindow::clear_input_handler`.

`PlatformInputHandler` carries two policy bits in addition to the callback
object:

- `accepts_text_input`, from `InputHandler::accepts_text_input`,
- `uses_manual_focus`, from whether the focused element was registered through
  `.manual_focus()`.

Normal editable handlers auto-request the soft keyboard when newly focused.
Manual-focus handlers still receive text input, but they do not auto-show the
keyboard. They must call `Window::show_soft_keyboard()` when they want a
keyboard session.

On iOS, input handlers use `TEXT_INTERACTION_EDITABLE` only when the handler
accepts text input, does not use manual focus, and the view is first responder.
This mode routes keyboard and IME callbacks to the input handler but does not
install UIKit's editable `UITextInteraction`, does not answer touch-selection
geometry, and suppresses UIKit's edit menu. When editable text is desired but
the view is not first responder, iOS falls back to `TEXT_INTERACTION_NONEDITABLE`
if a selection handler exists. This keeps read-only selection available without
showing an editable caret.

`keyboard_session_requested` is still needed on iOS to preserve an explicit
keyboard request until the input handler arrives. It is cleared when an
existing input handler is cleared so stale requests do not keep the responder
active.

Terminal-like views that own focus manually should use `.manual_focus()` on the
focus-tracked element. They can still provide a normal input handler for PTY
keyboard input, but keyboard presentation is explicit. If terminal content
later supports read-only native selection, it should register terminal text as
a `SelectionArea` document and keep PTY keyboard input in the separate
input-handler path.
