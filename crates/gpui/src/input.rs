use crate::{
    App, Bounds, Context, Entity, InputHandler, Pixels, PlatformTextInputTraits, UTF16Selection,
    Window,
};
use std::ops::Range;

/// Implement this trait to allow views to handle textual input when implementing an editor, field, etc.
///
/// Once your view implements this trait, you can use it to construct an [`ElementInputHandler<V>`].
/// This input handler can then be assigned during paint by calling [`Window::handle_input`].
///
/// See [`InputHandler`] for details on how to implement each method.
pub trait EntityInputHandler: 'static + Sized {
    /// See [`InputHandler::text_for_range`] for details
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String>;

    /// See [`InputHandler::text_len_utf16`] for details
    fn text_len_utf16(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        let mut adjusted_range = None;
        let text = self.text_for_range(usize::MAX..usize::MAX, &mut adjusted_range, window, cx)?;
        Some(adjusted_range.map_or_else(|| text.encode_utf16().count(), |range| range.end))
    }

    /// See [`InputHandler::selected_text_range`] for details
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection>;

    /// See [`InputHandler::set_selected_text_range`] for details
    fn set_selected_text_range(
        &mut self,
        _range: Range<usize>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    /// See [`InputHandler::marked_text_range`] for details
    fn marked_text_range(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>>;

    /// See [`InputHandler::unmark_text`] for details
    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>);

    /// See [`InputHandler::replace_text_in_range`] for details
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// See [`InputHandler::replace_and_mark_text_in_range`] for details
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// See [`InputHandler::should_change_text_in_range`] for details
    fn should_change_text_in_range(
        &mut self,
        _replacement_range: Option<Range<usize>>,
        _text: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> bool {
        true
    }

    /// See [`InputHandler::insert_text`] for details
    fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, text, window, cx);
    }

    /// See [`InputHandler::replace_range`] for details
    fn replace_range(
        &mut self,
        replacement_range: Range<usize>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(Some(replacement_range), text, window, cx);
    }

    /// See [`InputHandler::insert_dictation_result_placeholder`] for details
    fn insert_dictation_result_placeholder(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    /// See [`InputHandler::remove_dictation_result_placeholder`] for details
    fn remove_dictation_result_placeholder(
        &mut self,
        _will_insert_result: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    /// See [`InputHandler::insert_dictation_result`] for details
    fn insert_dictation_result(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, text, window, cx);
    }

    /// See [`InputHandler::dictation_recording_did_end`] for details
    fn dictation_recording_did_end(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// See [`InputHandler::dictation_recognition_failed`] for details
    fn dictation_recognition_failed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// See [`InputHandler::bounds_for_range`] for details
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>>;

    /// See [`InputHandler::character_index_for_point`] for details
    fn character_index_for_point(
        &mut self,
        point: crate::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize>;

    /// See [`InputHandler::accepts_text_input`] for details
    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        true
    }

    /// See [`InputHandler::text_input_traits`] for details
    fn text_input_traits(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> PlatformTextInputTraits {
        PlatformTextInputTraits::default()
    }

    /// See [`InputHandler::keyboard_accessory`] for details
    fn keyboard_accessory(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        false
    }

    /// See [`InputHandler::handle_keyboard_accessory_action`] for details
    fn handle_keyboard_accessory_action(
        &mut self,
        _action: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> bool {
        false
    }
}

/// The canonical implementation of [`crate::PlatformInputHandler`]. Call [`Window::handle_input`]
/// with an instance during your element's paint.
pub struct ElementInputHandler<V> {
    view: Entity<V>,
    element_bounds: Bounds<Pixels>,
}

impl<V: 'static> ElementInputHandler<V> {
    /// Used in [`Element::paint`][element_paint] with the element's bounds, a `Window`, and a `App` context.
    ///
    /// [element_paint]: crate::Element::paint
    pub fn new(element_bounds: Bounds<Pixels>, view: Entity<V>) -> Self {
        ElementInputHandler {
            view,
            element_bounds,
        }
    }
}

impl<V: EntityInputHandler> InputHandler for ElementInputHandler<V> {
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.view.update(cx, |view, cx| {
            view.selected_text_range(ignore_disabled_input, window, cx)
        })
    }

    fn set_selected_text_range(&mut self, range: Range<usize>, window: &mut Window, cx: &mut App) {
        self.view.update(cx, |view, cx| {
            view.set_selected_text_range(range, window, cx)
        });
    }

    fn marked_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.view
            .update(cx, |view, cx| view.marked_text_range(window, cx))
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        self.view.update(cx, |view, cx| {
            view.text_for_range(range_utf16, adjusted_range, window, cx)
        })
    }

    fn text_len_utf16(&mut self, window: &mut Window, cx: &mut App) -> Option<usize> {
        self.view
            .update(cx, |view, cx| view.text_len_utf16(window, cx))
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.replace_text_in_range(replacement_range, text, window, cx)
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(
                range_utf16,
                new_text,
                new_selected_range,
                window,
                cx,
            )
        });
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.view
            .update(cx, |view, cx| view.unmark_text(window, cx));
    }

    fn should_change_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        self.view.update(cx, |view, cx| {
            view.should_change_text_in_range(replacement_range, text, window, cx)
        })
    }

    fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut App) {
        self.view
            .update(cx, |view, cx| view.insert_text(text, window, cx));
    }

    fn replace_range(
        &mut self,
        replacement_range: Range<usize>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.replace_range(replacement_range, text, window, cx)
        });
    }

    fn insert_dictation_result_placeholder(&mut self, window: &mut Window, cx: &mut App) {
        self.view.update(cx, |view, cx| {
            view.insert_dictation_result_placeholder(window, cx)
        });
    }

    fn remove_dictation_result_placeholder(
        &mut self,
        will_insert_result: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.remove_dictation_result_placeholder(will_insert_result, window, cx)
        });
    }

    fn insert_dictation_result(&mut self, text: &str, window: &mut Window, cx: &mut App) {
        self.view.update(cx, |view, cx| {
            view.insert_dictation_result(text, window, cx)
        });
    }

    fn dictation_recording_did_end(&mut self, window: &mut Window, cx: &mut App) {
        self.view
            .update(cx, |view, cx| view.dictation_recording_did_end(window, cx));
    }

    fn dictation_recognition_failed(&mut self, window: &mut Window, cx: &mut App) {
        self.view
            .update(cx, |view, cx| view.dictation_recognition_failed(window, cx));
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.view.update(cx, |view, cx| {
            view.bounds_for_range(range_utf16, self.element_bounds, window, cx)
        })
    }

    fn character_index_for_point(
        &mut self,
        point: crate::Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.view.update(cx, |view, cx| {
            view.character_index_for_point(point, window, cx)
        })
    }

    fn accepts_text_input(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.view
            .update(cx, |view, cx| view.accepts_text_input(window, cx))
    }

    fn text_input_traits(&mut self, window: &mut Window, cx: &mut App) -> PlatformTextInputTraits {
        self.view
            .update(cx, |view, cx| view.text_input_traits(window, cx))
    }

    fn keyboard_accessory(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.view
            .update(cx, |view, cx| view.keyboard_accessory(window, cx))
    }

    fn handle_keyboard_accessory_action(
        &mut self,
        action: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        self.view.update(cx, |view, cx| {
            view.handle_keyboard_accessory_action(action, window, cx)
        })
    }

    fn prefers_ime_for_printable_keys(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.view
            .update(cx, |view, cx| view.accepts_text_input(window, cx))
    }
}
