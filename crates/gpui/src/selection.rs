use crate::{
    Action, AnyElement, App, Bounds, Element, ElementId, EntityId, GlobalElementId, InputHandler,
    InspectorElementId, IntoElement, LayoutId, Pixels, Point, SharedString, TextLayout,
    UTF16Selection, Window,
};
use smallvec::SmallVec;
use std::ops::Range;

/// Wrap a subtree in a logical text-selection boundary.
///
/// This establishes the GPUI-side identity for selectable content without
/// routing selection through the editable input system.
#[track_caller]
pub fn selection_area(child: impl IntoElement) -> SelectionAreaElement {
    SelectionAreaElement::new(child)
}

/// Logical metadata for a selectable subtree.
///
/// A selection area owns stable identity and any custom selection-scoped menu
/// actions. Platform selection presenters can bind native UI to this logical
/// boundary.
#[derive(Clone)]
pub struct SelectionArea {
    id: ElementId,
    actions: SmallVec<[SelectionAction; 4]>,
}

impl SelectionArea {
    /// Create a new logical selection area with the given element identity.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            actions: SmallVec::new(),
        }
    }

    /// Returns the element identity for this selection area.
    pub fn id(&self) -> &ElementId {
        &self.id
    }

    /// Returns the custom actions exposed for the current selection.
    pub fn actions(&self) -> &[SelectionAction] {
        &self.actions
    }

    /// Append a custom action to the native selection menu.
    pub fn action(mut self, name: impl Into<SharedString>, action: impl Action) -> Self {
        self.actions.push(SelectionAction::new(name, action));
        self
    }

    /// Extend the selection menu with pre-built actions.
    pub fn with_actions(mut self, actions: impl IntoIterator<Item = SelectionAction>) -> Self {
        self.actions.extend(actions);
        self
    }
}

/// A custom menu action attached to a selection area.
pub struct SelectionAction {
    name: SharedString,
    action: Box<dyn Action>,
}

impl SelectionAction {
    /// Create a new selection-scoped action.
    pub fn new(name: impl Into<SharedString>, action: impl Action) -> Self {
        Self {
            name: name.into(),
            action: Box::new(action),
        }
    }

    /// Returns the title that should appear in the native selection menu.
    pub fn name(&self) -> &SharedString {
        &self.name
    }

    /// Returns the GPUI action to dispatch when this menu item is chosen.
    pub fn action(&self) -> &dyn Action {
        self.action.as_ref()
    }
}

impl Clone for SelectionAction {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            action: self.action.boxed_clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ActiveSelectionArea {
    selection_area: SelectionArea,
    global_id: Option<GlobalElementId>,
}

impl ActiveSelectionArea {
    pub(crate) fn new(selection_area: SelectionArea, global_id: Option<GlobalElementId>) -> Self {
        Self {
            selection_area,
            global_id,
        }
    }

    pub(crate) fn selection_area(&self) -> &SelectionArea {
        &self.selection_area
    }

    pub(crate) fn global_id(&self) -> Option<&GlobalElementId> {
        self.global_id.as_ref()
    }
}

/// A selection area that was active during the current rendered frame.
#[derive(Clone)]
pub struct RegisteredSelectionArea {
    selection_area: SelectionArea,
    global_id: Option<GlobalElementId>,
    view_id: EntityId,
}

impl RegisteredSelectionArea {
    pub(crate) fn new(
        selection_area: SelectionArea,
        global_id: Option<GlobalElementId>,
        view_id: EntityId,
    ) -> Self {
        Self {
            selection_area,
            global_id,
            view_id,
        }
    }

    /// Returns the logical selection-area metadata for this registration.
    pub fn selection_area(&self) -> &SelectionArea {
        &self.selection_area
    }

    /// Returns the globally unique element identity for this area, if available.
    pub fn global_id(&self) -> Option<&GlobalElementId> {
        self.global_id.as_ref()
    }

    /// Returns the owning view for the registered selection area.
    pub fn view_id(&self) -> EntityId {
        self.view_id
    }
}

/// A text fragment that participates in selection inside a `SelectionArea`.
#[derive(Clone)]
pub struct TextSelectionFragment {
    text: SharedString,
    layout: TextLayout,
    order: Option<u64>,
    separator_after: SharedString,
}

impl TextSelectionFragment {
    /// Create a new text-layout-backed selection fragment.
    pub fn new(text: impl Into<SharedString>, layout: TextLayout) -> Self {
        Self {
            text: text.into(),
            layout,
            order: None,
            separator_after: SharedString::default(),
        }
    }

    /// Override this fragment's ordering within the containing selection area.
    pub fn order(mut self, order: u64) -> Self {
        self.order = Some(order);
        self
    }

    /// Append text after this fragment when copying across fragment boundaries.
    pub fn separator_after(mut self, separator_after: impl Into<SharedString>) -> Self {
        self.separator_after = separator_after.into();
        self
    }

    /// Returns the fragment text.
    pub fn text(&self) -> &SharedString {
        &self.text
    }

    /// Returns the fragment layout.
    pub fn layout(&self) -> &TextLayout {
        &self.layout
    }

    /// Returns the explicit ordering for this fragment, if any.
    pub fn explicit_order(&self) -> Option<u64> {
        self.order
    }

    /// Returns the separator appended after this fragment during copy serialization.
    pub fn trailing_separator(&self) -> &SharedString {
        &self.separator_after
    }
}

/// A text fragment registered during the current rendered frame.
#[derive(Clone)]
pub struct RegisteredTextSelectionFragment {
    active_area: ActiveSelectionArea,
    view_id: EntityId,
    fragment: TextSelectionFragment,
}

impl RegisteredTextSelectionFragment {
    pub(crate) fn new(
        active_area: ActiveSelectionArea,
        view_id: EntityId,
        fragment: TextSelectionFragment,
    ) -> Self {
        Self {
            active_area,
            view_id,
            fragment,
        }
    }

    /// Returns the logical selection area that owns this fragment.
    pub fn selection_area(&self) -> &SelectionArea {
        self.active_area.selection_area()
    }

    /// Returns the global element identity for the owning selection area, if available.
    pub fn selection_area_global_id(&self) -> Option<&GlobalElementId> {
        self.active_area.global_id()
    }

    /// Returns the view that painted this fragment.
    pub fn view_id(&self) -> EntityId {
        self.view_id
    }

    /// Returns the registered fragment data.
    pub fn fragment(&self) -> &TextSelectionFragment {
        &self.fragment
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum SelectionAreaKey {
    Global(GlobalElementId),
    ViewScoped(EntityId, ElementId),
}

impl SelectionAreaKey {
    fn from_fragment(fragment: &RegisteredTextSelectionFragment) -> Self {
        if let Some(global_id) = fragment.selection_area_global_id() {
            Self::Global(global_id.clone())
        } else {
            Self::ViewScoped(fragment.view_id(), fragment.selection_area().id().clone())
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct SelectionState {
    pub(crate) active_area: Option<SelectionAreaKey>,
    pub(crate) range_utf16: Option<Range<usize>>,
    pub(crate) reversed: bool,
}

#[derive(Clone)]
struct SelectionDocumentFragment {
    fragment: RegisteredTextSelectionFragment,
    content_start_utf16: usize,
    content_end_utf16: usize,
    separator_end_utf16: usize,
}

impl SelectionDocumentFragment {
    fn content_text(&self) -> &str {
        self.fragment.fragment().text().as_ref()
    }

    fn separator_text(&self) -> &str {
        self.fragment.fragment().trailing_separator().as_ref()
    }

    fn local_utf16_range(&self, range_utf16: &Range<usize>) -> Option<Range<usize>> {
        if range_utf16.start >= self.content_end_utf16
            || range_utf16.end <= self.content_start_utf16
        {
            return None;
        }

        Some(
            range_utf16.start.max(self.content_start_utf16) - self.content_start_utf16
                ..range_utf16.end.min(self.content_end_utf16) - self.content_start_utf16,
        )
    }

    fn utf16_index_for_point(&self, point: Point<Pixels>) -> usize {
        let index = self
            .fragment
            .fragment()
            .layout()
            .index_for_position(point)
            .unwrap_or_else(|index| index);
        self.content_start_utf16 + byte_offset_to_utf16(self.content_text(), index)
    }

    fn contains_point(&self, point: Point<Pixels>) -> bool {
        self.fragment.fragment().layout().bounds().contains(&point)
    }
}

struct SelectionDocument {
    key: SelectionAreaKey,
    fragments: Vec<SelectionDocumentFragment>,
    len_utf16: usize,
}

impl SelectionDocument {
    fn all(window: &Window) -> Vec<Self> {
        let mut grouped: Vec<(
            SelectionAreaKey,
            Vec<(usize, RegisteredTextSelectionFragment)>,
        )> = Vec::new();

        for (source_index, fragment) in window.selection_fragments().iter().cloned().enumerate() {
            let key = SelectionAreaKey::from_fragment(&fragment);
            if let Some((_, fragments)) = grouped
                .iter_mut()
                .find(|(existing_key, _)| *existing_key == key)
            {
                fragments.push((source_index, fragment));
            } else {
                grouped.push((key, vec![(source_index, fragment)]));
            }
        }

        grouped
            .into_iter()
            .map(|(key, fragments)| Self::from_fragments(key, fragments))
            .collect()
    }

    fn active(window: &Window) -> Option<Self> {
        let active_key = window.selection_state.active_area.as_ref()?;
        Self::all(window)
            .into_iter()
            .find(|document| &document.key == active_key)
    }

    fn from_fragments(
        key: SelectionAreaKey,
        mut fragments: Vec<(usize, RegisteredTextSelectionFragment)>,
    ) -> Self {
        fragments.sort_by_key(|(source_index, fragment)| {
            (
                fragment
                    .fragment()
                    .explicit_order()
                    .unwrap_or(*source_index as u64),
                *source_index,
            )
        });

        let mut len_utf16 = 0;
        let fragments = fragments
            .into_iter()
            .map(|(_, fragment)| {
                let content_start_utf16 = len_utf16;
                len_utf16 += utf16_len(fragment.fragment().text().as_ref());
                let content_end_utf16 = len_utf16;
                len_utf16 += utf16_len(fragment.fragment().trailing_separator().as_ref());
                let separator_end_utf16 = len_utf16;

                SelectionDocumentFragment {
                    fragment,
                    content_start_utf16,
                    content_end_utf16,
                    separator_end_utf16,
                }
            })
            .collect();

        Self {
            key,
            fragments,
            len_utf16,
        }
    }

    fn clamped_range(&self, range_utf16: Range<usize>) -> Range<usize> {
        let start = range_utf16.start.min(self.len_utf16);
        let end = range_utf16.end.min(self.len_utf16);
        start.min(end)..end.max(start)
    }

    fn text_for_range(&self, range_utf16: Range<usize>) -> Option<String> {
        let range_utf16 = self.clamped_range(range_utf16);
        if range_utf16.start > range_utf16.end {
            return None;
        }

        let full_text = self.serialize_text();
        let start = utf16_offset_to_byte(&full_text, range_utf16.start);
        let end = utf16_offset_to_byte(&full_text, range_utf16.end);
        Some(full_text[start..end].to_string())
    }

    fn serialize_text(&self) -> String {
        let mut text = String::new();
        for fragment in &self.fragments {
            text.push_str(fragment.content_text());
            text.push_str(fragment.separator_text());
        }
        text
    }

    fn character_index_for_point(&self, point: Point<Pixels>) -> Option<usize> {
        self.fragments
            .iter()
            .find(|fragment| fragment.contains_point(point))
            .map(|fragment| fragment.utf16_index_for_point(point).min(self.len_utf16))
    }

    fn bounds_for_range(&self, range_utf16: Range<usize>) -> Option<Bounds<Pixels>> {
        let range_utf16 = self.clamped_range(range_utf16);
        if range_utf16.is_empty() {
            return self.caret_bounds_for_offset(range_utf16.start);
        }

        self.rects_for_range(range_utf16).into_iter().next()
    }

    fn rects_for_range(&self, range_utf16: Range<usize>) -> SmallVec<[Bounds<Pixels>; 4]> {
        let range_utf16 = self.clamped_range(range_utf16);
        if range_utf16.is_empty() {
            return SmallVec::new();
        }

        let mut rects = SmallVec::new();
        for fragment in &self.fragments {
            let Some(local_utf16_range) = fragment.local_utf16_range(&range_utf16) else {
                continue;
            };

            let local_start_byte =
                utf16_offset_to_byte(fragment.content_text(), local_utf16_range.start);
            let local_end_byte =
                utf16_offset_to_byte(fragment.content_text(), local_utf16_range.end);

            rects.extend(
                fragment
                    .fragment
                    .fragment()
                    .layout()
                    .rects_for_range(local_start_byte..local_end_byte),
            );
        }

        rects
    }

    fn caret_bounds_for_offset(&self, offset_utf16: usize) -> Option<Bounds<Pixels>> {
        let offset_utf16 = offset_utf16.min(self.len_utf16);
        let fragment = self
            .fragments
            .iter()
            .find(|fragment| offset_utf16 <= fragment.separator_end_utf16)
            .or_else(|| self.fragments.last())?;

        let local_utf16 = offset_utf16
            .clamp(fragment.content_start_utf16, fragment.content_end_utf16)
            - fragment.content_start_utf16;
        let text = fragment.content_text();
        let layout = fragment.fragment.fragment().layout();
        let local_byte = utf16_offset_to_byte(text, local_utf16);

        let rect = if local_byte < text.len() {
            next_char_byte(text, local_byte).and_then(|next_byte| {
                layout
                    .rects_for_range(local_byte..next_byte)
                    .into_iter()
                    .next()
                    .map(|bounds| {
                        Bounds::new(
                            bounds.origin,
                            crate::size(crate::px(0.0), bounds.size.height),
                        )
                    })
            })
        } else {
            previous_char_byte(text, local_byte).and_then(|previous_byte| {
                layout
                    .rects_for_range(previous_byte..local_byte)
                    .into_iter()
                    .last()
                    .map(|bounds| {
                        Bounds::new(
                            crate::point(bounds.right(), bounds.origin.y),
                            crate::size(crate::px(0.0), bounds.size.height),
                        )
                    })
            })
        };

        rect.or_else(|| {
            layout.position_for_index(local_byte).map(|position| {
                Bounds::new(
                    position,
                    crate::size(crate::px(0.0), layout.bounds().size.height),
                )
            })
        })
    }
}

/// Window-level read-only selection bridge.
///
/// Platform backends expose this through their native text-input surface so
/// system selection UI, such as iOS noneditable `UITextInteraction`, can query
/// text, ranges, hit testing, and selection rects for GPUI `SelectionArea`s.
#[derive(Default)]
pub(crate) struct WindowSelectionHandler;

impl InputHandler for WindowSelectionHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        let document = SelectionDocument::active(window)?;
        let range = window
            .selection_state
            .range_utf16
            .clone()
            .map(|range| document.clamped_range(range))
            .unwrap_or(0..0);
        window.selection_state.range_utf16 = Some(range.clone());

        Some(UTF16Selection {
            range,
            reversed: window.selection_state.reversed,
        })
    }

    fn marked_text_range(&mut self, _window: &mut Window, _cx: &mut App) -> Option<Range<usize>> {
        None
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        let document = SelectionDocument::active(window)?;
        let range_utf16 = document.clamped_range(range_utf16);
        *adjusted_range = Some(range_utf16.clone());
        document.text_for_range(range_utf16)
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<Range<usize>>,
        _text: &str,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        _new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut App) {}

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        _cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        SelectionDocument::active(window)?.bounds_for_range(range_utf16)
    }

    fn rects_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        _cx: &mut App,
    ) -> SmallVec<[Bounds<Pixels>; 4]> {
        SelectionDocument::active(window)
            .map(|document| document.rects_for_range(range_utf16))
            .unwrap_or_default()
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        let documents = SelectionDocument::all(window);
        let (key, index) = documents.iter().find_map(|document| {
            document
                .character_index_for_point(point)
                .map(|index| (&document.key, index))
        })?;

        let area_changed = window.selection_state.active_area.as_ref() != Some(key);
        window.selection_state.active_area = Some(key.clone());
        if area_changed || window.selection_state.range_utf16.is_none() {
            window.selection_state.range_utf16 = Some(index..index);
            window.selection_state.reversed = false;
        }

        Some(index)
    }

    fn accepts_text_input(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        false
    }

    fn set_selected_text_range(&mut self, range: Range<usize>, window: &mut Window, _cx: &mut App) {
        let Some(document) = SelectionDocument::active(window) else {
            return;
        };
        window.selection_state.range_utf16 = Some(document.clamped_range(range));
        window.selection_state.reversed = false;
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn utf16_offset_to_byte(text: &str, offset_utf16: usize) -> usize {
    if offset_utf16 == 0 {
        return 0;
    }

    let mut utf16_offset = 0;
    for (byte_offset, character) in text.char_indices() {
        if utf16_offset >= offset_utf16 {
            return byte_offset;
        }
        let next_utf16_offset = utf16_offset + character.len_utf16();
        if offset_utf16 < next_utf16_offset {
            return byte_offset;
        }
        utf16_offset = next_utf16_offset;
    }
    text.len()
}

fn byte_offset_to_utf16(text: &str, byte_offset: usize) -> usize {
    let mut utf16_offset = 0;
    for (character_offset, character) in text.char_indices() {
        if character_offset >= byte_offset {
            break;
        }
        utf16_offset += character.len_utf16();
    }
    utf16_offset
}

fn previous_char_byte(text: &str, byte_offset: usize) -> Option<usize> {
    text.char_indices()
        .map(|(offset, _)| offset)
        .take_while(|offset| *offset < byte_offset)
        .last()
}

fn next_char_byte(text: &str, byte_offset: usize) -> Option<usize> {
    let mut offsets = text.char_indices().map(|(offset, _)| offset).peekable();
    while let Some(offset) = offsets.next() {
        if offset == byte_offset {
            return offsets.peek().copied().or(Some(text.len()));
        }
    }
    None
}

/// An element wrapper that marks its subtree as a logical selection area.
pub struct SelectionAreaElement {
    selection_area: SelectionArea,
    child: AnyElement,
    source_location: &'static core::panic::Location<'static>,
}

impl SelectionAreaElement {
    /// Create a selection-area wrapper around the provided child subtree.
    #[track_caller]
    pub fn new(child: impl IntoElement) -> Self {
        let source_location = core::panic::Location::caller();
        Self {
            selection_area: SelectionArea::new(ElementId::CodeLocation(*source_location)),
            child: child.into_any_element(),
            source_location,
        }
    }

    /// Override the selection area's stable identity.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.selection_area.id = id.into();
        self
    }

    /// Append a custom selection action to this area.
    pub fn action(mut self, name: impl Into<SharedString>, action: impl Action) -> Self {
        self.selection_area
            .actions
            .push(SelectionAction::new(name, action));
        self
    }

    /// Extend this selection area with pre-built custom actions.
    pub fn with_actions(mut self, actions: impl IntoIterator<Item = SelectionAction>) -> Self {
        self.selection_area.actions.extend(actions);
        self
    }

    /// Returns the logical selection-area metadata for this wrapper.
    pub fn selection_area(&self) -> &SelectionArea {
        &self.selection_area
    }
}

impl Element for SelectionAreaElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.selection_area.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        Some(self.source_location)
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout_id =
            window.with_selection_area(Some(&self.selection_area), global_id, |window| {
                self.child.request_layout(window, cx)
            });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        window.with_selection_area(Some(&self.selection_area), global_id, |window| {
            self.child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_selection_area(Some(&self.selection_area), global_id, |window| {
            window.register_selection_area(&self.selection_area, global_id);
            self.child.paint(window, cx);
        });
    }
}

impl IntoElement for SelectionAreaElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{SelectionDocument, SelectionState};
    use crate::{
        AppContext, Context, IntoElement, ParentElement, Render, StyledText, TestAppContext,
        Window, div, point, px, selection_area,
    };

    struct SelectionTestView;

    impl Render for SelectionTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            selection_area(
                div()
                    .child(
                        StyledText::new("hello")
                            .selectable()
                            .selection_separator_after(" "),
                    )
                    .child(StyledText::new("world").selectable()),
            )
        }
    }

    struct DivSelectionAreaTestView;

    impl Render for DivSelectionAreaTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(
                    div()
                        .child(StyledText::new("alpha").selectable())
                        .selection_area(),
                )
                .child(
                    div()
                        .child(StyledText::new("beta").selectable())
                        .selection_area(),
                )
        }
    }

    struct DivSelectionContainerTestView;

    impl Render for DivSelectionContainerTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(
                    div()
                        .child(StyledText::new("gamma").selectable())
                        .selection_container(),
                )
                .child(
                    div()
                        .child(StyledText::new("delta").selectable())
                        .selection_container(),
                )
        }
    }

    #[gpui::test]
    fn registers_selection_handler_before_interaction(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(|_| SelectionTestView))
                .unwrap()
        });
        cx.run_until_parked();

        let test_window = cx.test_window(*window);
        assert!(test_window.has_selection_handler());
    }

    #[gpui::test]
    fn selection_handler_reads_multi_fragment_selection_area(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(|_| SelectionTestView))
                .unwrap()
        });
        cx.run_until_parked();

        window
            .update(cx, |_, window, _| {
                let document = SelectionDocument::all(window).into_iter().next().unwrap();
                window.selection_state = SelectionState {
                    active_area: Some(document.key.clone()),
                    range_utf16: Some(0..11),
                    reversed: false,
                };
            })
            .unwrap();

        let test_window = cx.test_window(*window);
        let mut handler = test_window.take_selection_handler_for_test().unwrap();
        let mut adjusted_range = None;

        assert_eq!(handler.selected_text_range(false).unwrap().range, 0..11);
        assert_eq!(
            handler
                .text_for_range(0..11, &mut adjusted_range)
                .as_deref(),
            Some("hello world")
        );
        assert_eq!(adjusted_range, Some(0..11));
        assert!(handler.bounds_for_range(0..11).is_some());
        assert!(!handler.rects_for_range(0..11).is_empty());
    }

    #[gpui::test]
    fn selection_handler_ignores_points_outside_selection_area(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(|_| SelectionTestView))
                .unwrap()
        });
        cx.run_until_parked();

        let test_window = cx.test_window(*window);
        let mut handler = test_window.take_selection_handler_for_test().unwrap();

        assert_eq!(
            handler.character_index_for_point(point(px(10_000.0), px(10_000.0))),
            None
        );
    }

    #[gpui::test]
    fn div_selection_area_uses_distinct_global_ids(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| DivSelectionAreaTestView)
            })
            .unwrap()
        });
        cx.run_until_parked();

        window
            .update(cx, |_, window, _| {
                let mut texts = SelectionDocument::all(window)
                    .into_iter()
                    .map(|document| document.text_for_range(0..usize::MAX).unwrap())
                    .collect::<Vec<_>>();
                texts.sort();
                assert_eq!(texts, vec!["alpha".to_string(), "beta".to_string()]);

                let global_ids = window
                    .selection_areas()
                    .iter()
                    .map(|area| area.global_id().unwrap().clone())
                    .collect::<Vec<_>>();
                assert_eq!(global_ids.len(), 2);
                assert_ne!(global_ids[0], global_ids[1]);
            })
            .unwrap();
    }

    #[gpui::test]
    fn div_selection_container_uses_distinct_global_ids(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| DivSelectionContainerTestView)
            })
            .unwrap()
        });
        cx.run_until_parked();

        window
            .update(cx, |_, window, _| {
                let mut texts = SelectionDocument::all(window)
                    .into_iter()
                    .map(|document| document.text_for_range(0..usize::MAX).unwrap())
                    .collect::<Vec<_>>();
                texts.sort();
                assert_eq!(texts, vec!["delta".to_string(), "gamma".to_string()]);

                let global_ids = window
                    .selection_areas()
                    .iter()
                    .map(|area| area.global_id().unwrap().clone())
                    .collect::<Vec<_>>();
                assert_eq!(global_ids.len(), 2);
                assert_ne!(global_ids[0], global_ids[1]);
            })
            .unwrap();
    }
}
