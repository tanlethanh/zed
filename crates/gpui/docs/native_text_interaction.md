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
GPUI splits the model into three parts:

- `SelectionArea`, a logical document boundary for normal element-tree text,
- `Selectable`, a read-only leaf document for custom-painted surfaces,
- `WindowSelectionHandler`, the single native selection host for the window.

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

Inside a `SelectionArea`, `StyledText` fragments opt in with:

- `.selectable()` to register the text with the nearest selection area,
- `.selection_order(order)` to override paint-order document ordering,
- `.selection_separator_after(text)` to append copied text across fragment
  boundaries, such as a space or newline.

Custom-painted elements that cannot render through `StyledText` implement
`Selectable` instead. During paint, they call
`Window::register_selectable(id, selectable, text_bounds, selection_area_bounds)`.
The stable id keeps the active range attached to the same custom surface across
repaints. The selectable answers the same native read-only document queries
directly: text for UTF-16 range, hit testing, first range bounds, range rects,
range-change notifications, clearing, and custom selection actions. The active
range itself remains owned by the window selection state. This keeps custom
surfaces selectable without making them editable input handlers.

During layout, prepaint, and paint, `SelectionAreaElement` pushes the current
selection area onto the window stack. During paint it registers the area
identity and each selectable text fragment painted inside the area. Cached
subtree reuse must replay area, fragment, and `Selectable` registrations.

After drawing roots, the window installs `PlatformWindow::set_selection_handler`
if any selection fragments or `Selectable`s were registered. If the frame has no
read-only selection content, GPUI clears the selection handler and resets
read-only selection state. There is still only one platform selection handler
for the window; direct `Selectable`s do not become platform input handlers.

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
When hit testing starts on a registered `Selectable`, `WindowSelectionHandler`
marks that selectable active and routes subsequent native range, text, rect,
clear, and action queries to it until the selection is cleared or another
selection target becomes active.

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

- built-in fragment registration is implemented for `StyledText`,
- custom-painted content must provide its own `Selectable` document model and
  frame-local hit bounds,
- selection-scoped custom actions are presented by the iOS edit menu when a
  read-only `SelectionArea` or `Selectable` is active,
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

On iOS, input handlers use `TEXT_INTERACTION_EDITABLE` when the handler accepts
text input and the view is first responder. This mode routes keyboard and IME
callbacks to the input handler. Normal editable handlers suppress UIKit's native
touch selection and edit menu, because GPUI text fields still own their own
selection model. A handler that explicitly returns `handles_native_selection()`
can opt into native range, text, rect, copy, and custom selection-action queries
while staying in editable mode.

When editable text is desired but the view is not first responder, iOS falls
back to `TEXT_INTERACTION_NONEDITABLE` if a selection handler exists. This keeps
read-only selection available without showing an editable caret.

`keyboard_session_requested` is still needed on iOS to preserve an explicit
keyboard request until the input handler arrives. It is cleared when an
existing input handler is cleared so stale requests do not keep the responder
active.

Terminal-like views that own focus manually should use `.manual_focus()` on the
focus-tracked element. They can still provide a normal input handler for PTY
keyboard input, but keyboard presentation is explicit. Terminal content should
not register the painted grid as a passive `Selectable`, because ordinary text
hits would then compete with tap-to-focus keyboard behavior. Instead, a terminal
input handler can explicitly report `handles_native_selection()` for its
terminal surface. iOS then asks that same handler whether a native text
interaction point hits terminal output. GPUI must not
suppress the window touch stream from that hit test alone. Terminal input-native
selection is a long-press path; normal taps stay with GPUI keyboard activation,
and double tap should not start terminal selection. The native path owns range,
text, rect, copy, and custom selection-action callbacks once the long press
begins. Blank-cell paste should be implemented by the app as a separate native
edit menu, not by broadening GPUI text-selection policy. Ordinary terminal taps
should keep their existing focus/keyboard toggle
policy: unfocused taps focus and request the keyboard, focused taps with a
hidden keyboard request it again, and focused taps with a visible keyboard hide
it and clear focus. The output document remains read-only from the handler's
perspective: keyboard, IME, and dictation mutations must clear or ignore output
selection and continue through the terminal input path rather than replacing
terminal output. If a touch only dismisses an active native output selection,
iOS consumes that touch stream so it does not also trigger the terminal's normal
tap policy.
