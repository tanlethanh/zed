use crate::{
    Action, AnyElement, App, Bounds, Element, ElementId, EntityId, GlobalElementId, HitboxBehavior,
    InputHandler, InspectorElementId, IntoElement, LayoutId, Pixels, Point, SharedString,
    TextLayout, UTF16Selection, Window,
};
use smallvec::SmallVec;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

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

    /// Append a custom action with a platform-native image name.
    pub fn action_with_image(
        mut self,
        name: impl Into<SharedString>,
        image_name: impl Into<SharedString>,
        action: impl Action,
    ) -> Self {
        self.actions
            .push(SelectionAction::new(name, action).image(image_name));
        self
    }

    /// Append a custom action with a platform-native system image name.
    pub fn action_with_system_image(
        self,
        name: impl Into<SharedString>,
        system_image_name: impl Into<SharedString>,
        action: impl Action,
    ) -> Self {
        self.action_with_image(name, system_image_name, action)
    }

    /// Extend the selection menu with pre-built actions.
    pub fn with_actions(mut self, actions: impl IntoIterator<Item = SelectionAction>) -> Self {
        self.actions.extend(actions);
        self
    }
}

/// Native menu presentation metadata for a selection action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionActionPresentation {
    /// Title shown for this action in the native selection menu.
    pub name: SharedString,
    /// Optional platform-native image name shown beside this action.
    pub image_name: Option<SharedString>,
}

/// A custom menu action attached to a selection area.
pub struct SelectionAction {
    name: SharedString,
    image_name: Option<SharedString>,
    action: Box<dyn Action>,
}

impl SelectionAction {
    /// Create a new selection-scoped action.
    pub fn new(name: impl Into<SharedString>, action: impl Action) -> Self {
        Self {
            name: name.into(),
            image_name: None,
            action: Box::new(action),
        }
    }

    /// Set the platform-native image name shown beside this action.
    pub fn image(mut self, image_name: impl Into<SharedString>) -> Self {
        self.image_name = Some(image_name.into());
        self
    }

    /// Set the platform-native system image name shown beside this action.
    pub fn system_image(self, system_image_name: impl Into<SharedString>) -> Self {
        self.image(system_image_name)
    }

    /// Returns the title that should appear in the native selection menu.
    pub fn name(&self) -> &SharedString {
        &self.name
    }

    /// Returns the platform-native image name for the native selection menu.
    pub fn image_name(&self) -> Option<&SharedString> {
        self.image_name.as_ref()
    }

    /// Returns the native menu presentation metadata for this action.
    pub fn presentation(&self) -> SelectionActionPresentation {
        SelectionActionPresentation {
            name: self.name.clone(),
            image_name: self.image_name.clone(),
        }
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
            image_name: self.image_name.clone(),
            action: self.action.boxed_clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ActiveSelectionArea {
    selection_area: SelectionArea,
    global_id: Option<GlobalElementId>,
    bounds: Option<Bounds<Pixels>>,
    hitbox_range: Option<Range<usize>>,
}

impl ActiveSelectionArea {
    pub(crate) fn new(selection_area: SelectionArea, global_id: Option<GlobalElementId>) -> Self {
        Self {
            selection_area,
            global_id,
            bounds: None,
            hitbox_range: None,
        }
    }

    pub(crate) fn with_interaction_region(
        mut self,
        bounds: Bounds<Pixels>,
        hitbox_range: Range<usize>,
    ) -> Self {
        self.bounds = Some(bounds);
        self.hitbox_range = Some(hitbox_range);
        self
    }

    pub(crate) fn selection_area(&self) -> &SelectionArea {
        &self.selection_area
    }

    pub(crate) fn global_id(&self) -> Option<&GlobalElementId> {
        self.global_id.as_ref()
    }

    pub(crate) fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.bounds
    }

    pub(crate) fn hitbox_range(&self) -> Option<&Range<usize>> {
        self.hitbox_range.as_ref()
    }
}

/// A selection area that was active during the current rendered frame.
#[derive(Clone)]
pub struct RegisteredSelectionArea {
    selection_area: SelectionArea,
    global_id: Option<GlobalElementId>,
    view_id: EntityId,
    bounds: Option<Bounds<Pixels>>,
}

impl RegisteredSelectionArea {
    pub(crate) fn new(
        selection_area: SelectionArea,
        global_id: Option<GlobalElementId>,
        view_id: EntityId,
        bounds: Option<Bounds<Pixels>>,
    ) -> Self {
        Self {
            selection_area,
            global_id,
            view_id,
            bounds,
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

    /// Returns the rendered bounds for this selection area, if known.
    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.bounds
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

    pub(crate) fn selection_area_bounds(&self) -> Option<Bounds<Pixels>> {
        self.active_area.bounds()
    }

    pub(crate) fn selection_area_hitbox_range(&self) -> Option<&Range<usize>> {
        self.active_area.hitbox_range()
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

    fn selection_area_id(&self) -> Option<&ElementId> {
        match self {
            Self::Global(global_id) => global_id.0.last(),
            Self::ViewScoped(_, element_id) => Some(element_id),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct SelectionState {
    pub(crate) active_area: Option<SelectionAreaKey>,
    pub(crate) range_utf16: Option<Range<usize>>,
    pub(crate) reversed: bool,
}

/// Snapshot of a read-only selection inside a GPUI [`SelectionArea`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadOnlySelectionSnapshot {
    /// The selected area's stable element id.
    pub area_id: ElementId,
    /// The selected range in the area's flattened UTF-16 document.
    pub range_utf16: Range<usize>,
    /// The text covered by `range_utf16`.
    pub text: String,
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

    fn vertical_distance_to_point(&self, point: Point<Pixels>) -> Pixels {
        let bounds = self.fragment.fragment().layout().bounds();
        if point.y < bounds.top() {
            bounds.top() - point.y
        } else if point.y > bounds.bottom() {
            point.y - bounds.bottom()
        } else {
            Pixels::ZERO
        }
    }

    fn horizontal_distance_to_point(&self, point: Point<Pixels>) -> Pixels {
        let bounds = self.fragment.fragment().layout().bounds();
        if point.x < bounds.left() {
            bounds.left() - point.x
        } else if point.x > bounds.right() {
            point.x - bounds.right()
        } else {
            Pixels::ZERO
        }
    }
}

struct SelectionDocument {
    key: SelectionAreaKey,
    fragments: Vec<SelectionDocumentFragment>,
    len_utf16: usize,
    bounds: Option<Bounds<Pixels>>,
    hitbox_range: Option<Range<usize>>,
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
        let mut documents = Self::all(window);
        if let Some(index) = documents
            .iter()
            .position(|document| &document.key == active_key)
        {
            return Some(documents.remove(index));
        }

        let active_area_id = active_key.selection_area_id()?;
        let mut matching_documents = documents
            .into_iter()
            .filter(|document| document.key.selection_area_id() == Some(active_area_id));
        let document = matching_documents.next()?;
        matching_documents.next().is_none().then_some(document)
    }

    fn snapshot_for_range(&self, range_utf16: Range<usize>) -> Option<ReadOnlySelectionSnapshot> {
        let range_utf16 = self.clamped_range(range_utf16);
        if range_utf16.is_empty() {
            return None;
        }
        Some(ReadOnlySelectionSnapshot {
            area_id: self.key.selection_area_id()?.clone(),
            text: self.text_for_range(range_utf16.clone())?,
            range_utf16,
        })
    }

    fn actions(&self) -> SmallVec<[SelectionAction; 4]> {
        self.fragments
            .first()
            .map(|fragment| {
                fragment
                    .fragment
                    .selection_area()
                    .actions()
                    .iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
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
        let bounds = fragments
            .first()
            .and_then(|(_, fragment)| fragment.selection_area_bounds());
        let hitbox_range = fragments
            .first()
            .and_then(|(_, fragment)| fragment.selection_area_hitbox_range().cloned());

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
            bounds,
            hitbox_range,
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

    fn word_range_at(&self, offset_utf16: usize) -> Option<Range<usize>> {
        let text = self.serialize_text();
        word_range_at_utf16(&text, offset_utf16)
    }

    fn character_index_for_point(&self, point: Point<Pixels>, window: &Window) -> Option<usize> {
        if self.bounds.is_some_and(|bounds| !bounds.contains(&point)) {
            return None;
        }
        if self
            .hitbox_range
            .as_ref()
            .is_some_and(|range| selection_area_is_occluded_at_point(window, point, range))
        {
            return None;
        }

        self.fragments
            .iter()
            .find(|fragment| fragment.contains_point(point))
            .map(|fragment| fragment.utf16_index_for_point(point).min(self.len_utf16))
    }

    fn nearest_character_index_for_point(
        &self,
        point: Point<Pixels>,
        window: &Window,
    ) -> Option<usize> {
        if self.bounds.is_some_and(|bounds| !bounds.contains(&point)) {
            return None;
        }
        if self
            .hitbox_range
            .as_ref()
            .is_some_and(|range| selection_area_is_occluded_at_point(window, point, range))
        {
            return None;
        }

        self.character_index_for_point(point, window).or_else(|| {
            self.fragments
                .iter()
                .min_by(|a, b| {
                    a.vertical_distance_to_point(point)
                        .as_f32()
                        .total_cmp(&b.vertical_distance_to_point(point).as_f32())
                        .then_with(|| {
                            a.horizontal_distance_to_point(point)
                                .as_f32()
                                .total_cmp(&b.horizontal_distance_to_point(point).as_f32())
                        })
                        .then_with(|| a.content_start_utf16.cmp(&b.content_start_utf16))
                })
                .map(|fragment| fragment.utf16_index_for_point(point).min(self.len_utf16))
        })
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

pub(crate) fn active_read_only_selection(window: &Window) -> Option<ReadOnlySelectionSnapshot> {
    let document = SelectionDocument::active(window)?;
    document.snapshot_for_range(window.selection_state.range_utf16.clone()?)
}

fn cache_read_only_selection(
    window: &mut Window,
    document: &SelectionDocument,
    range_utf16: Range<usize>,
) {
    if let Some(selection) = document.snapshot_for_range(range_utf16) {
        window.latest_read_only_selection = Some(selection);
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
        let hit = documents.iter().find_map(|document| {
            document
                .character_index_for_point(point, window)
                .map(|index| (&document.key, index))
        });
        let (key, index) = hit?;

        let area_changed = window.selection_state.active_area.as_ref() != Some(key);
        window.selection_state.active_area = Some(key.clone());
        if area_changed || window.selection_state.range_utf16.is_none() {
            window.selection_state.range_utf16 = Some(index..index);
            window.selection_state.reversed = false;
        }

        Some(index)
    }

    fn nearest_character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        let active_document = SelectionDocument::active(window);
        let documents = SelectionDocument::all(window);
        active_document
            .into_iter()
            .chain(documents)
            .find_map(|document| document.nearest_character_index_for_point(point, window))
    }

    fn accepts_text_input(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        false
    }

    fn set_selected_text_range(&mut self, range: Range<usize>, window: &mut Window, _cx: &mut App) {
        let Some(document) = SelectionDocument::active(window) else {
            return;
        };
        let clamped_range = document.clamped_range(range.clone());
        window.selection_state.range_utf16 = Some(clamped_range);
        window.selection_state.reversed = false;
        cache_read_only_selection(window, &document, range);
    }

    fn clear_selected_text_range(&mut self, window: &mut Window, _cx: &mut App) {
        window.selection_state = SelectionState::default();
    }

    fn initial_native_selection_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        _cx: &mut App,
    ) -> Option<Range<usize>> {
        let document = SelectionDocument::active(window)?;
        let range = document.clamped_range(range);
        document.word_range_at(range.start).or(Some(range))
    }

    fn selection_actions(
        &mut self,
        window: &mut Window,
        _cx: &mut App,
    ) -> SmallVec<[SelectionAction; 4]> {
        let Some(document) = SelectionDocument::active(window) else {
            return SmallVec::new();
        };
        let Some(range_utf16) = window.selection_state.range_utf16.clone() else {
            return SmallVec::new();
        };
        if document.clamped_range(range_utf16).is_empty() {
            return SmallVec::new();
        }
        document.actions()
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn word_range_at_utf16(text: &str, offset_utf16: usize) -> Option<Range<usize>> {
    let offset_byte = utf16_offset_to_byte(text, offset_utf16);
    text.unicode_word_indices()
        .find(|(start, word)| *start <= offset_byte && offset_byte < start + word.len())
        .map(|(start, word)| {
            byte_offset_to_utf16(text, start)..byte_offset_to_utf16(text, start + word.len())
        })
}

fn selection_area_is_occluded_at_point(
    window: &Window,
    point: Point<Pixels>,
    hitbox_range: &Range<usize>,
) -> bool {
    let hitbox_start = hitbox_range.start.min(window.rendered_frame.hitboxes.len());
    let hitbox_end = hitbox_range.end.min(window.rendered_frame.hitboxes.len());

    let hit_test = window.rendered_frame.hit_test(point);
    if window.rendered_frame.hitboxes[hitbox_start..hitbox_end]
        .iter()
        .any(|hitbox| {
            matches!(
                hitbox.behavior,
                HitboxBehavior::BlockMouse | HitboxBehavior::BlockMouseExceptScroll
            ) && hitbox.interaction_bounds().contains(&point)
                && hit_test.ids.contains(&hitbox.id)
        })
    {
        return true;
    }

    if hitbox_end >= window.rendered_frame.hitboxes.len() {
        return false;
    }

    hit_test.ids.iter().any(|hitbox_id| {
        window.rendered_frame.hitboxes[hitbox_end..]
            .iter()
            .any(|hitbox| {
                hitbox.id == *hitbox_id
                    // Later normal hitboxes can be gesture affordances such as
                    // a drawer edge strip; only explicit blockers should
                    // suppress native selection.
                    && matches!(
                        hitbox.behavior,
                        HitboxBehavior::BlockMouse | HitboxBehavior::BlockMouseExceptScroll
                    )
            })
    })
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

/// Prepaint state used to preserve the hitboxes owned by a selection area subtree.
pub struct SelectionAreaPrepaintState {
    hitbox_range: Range<usize>,
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

    /// Append a custom selection action with a platform-native image name.
    pub fn action_with_image(
        mut self,
        name: impl Into<SharedString>,
        image_name: impl Into<SharedString>,
        action: impl Action,
    ) -> Self {
        self.selection_area
            .actions
            .push(SelectionAction::new(name, action).image(image_name));
        self
    }

    /// Append a custom selection action with a platform-native system image name.
    pub fn action_with_system_image(
        self,
        name: impl Into<SharedString>,
        system_image_name: impl Into<SharedString>,
        action: impl Action,
    ) -> Self {
        self.action_with_image(name, system_image_name, action)
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
    type PrepaintState = SelectionAreaPrepaintState;

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
        let hitbox_start = window.next_frame.hitboxes.len();
        window.with_selection_area(Some(&self.selection_area), global_id, |window| {
            self.child.prepaint(window, cx);
        });
        SelectionAreaPrepaintState {
            hitbox_range: hitbox_start..window.next_frame.hitboxes.len(),
        }
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_selection_area_region(
            &self.selection_area,
            global_id,
            bounds,
            prepaint.hitbox_range.clone(),
            |window| {
                window.register_selection_area(&self.selection_area, global_id);
                self.child.paint(window, cx);
            },
        );
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
    use super::{SelectionAreaKey, SelectionDocument, SelectionState, word_range_at_utf16};
    use crate::EntityId;
    use crate::{
        self as gpui, AnyWindowHandle, AppContext, Context, InputEvent, InteractiveElement,
        InteractiveText, IntoElement, Modifiers, ParentElement, Pixels, PointerButton,
        PointerDownEvent, PointerKind, PointerUpEvent, Render, Styled, StyledText, TestAppContext,
        TextLayout, Window, div, point, px, selection_area,
    };
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    struct SelectionTestView;

    #[test]
    fn initial_native_selection_expands_to_surrounding_word() {
        let text = "If a task needs more detail";

        assert_eq!(word_range_at_utf16(text, 11), Some(10..15));
        assert_eq!(&text[10..15], "needs");
    }

    #[test]
    fn initial_native_selection_preserves_non_word_characters() {
        let text = "needs, details";

        assert_eq!(word_range_at_utf16(text, 5), None);
        assert_eq!(word_range_at_utf16(text, 6), None);
    }

    #[test]
    fn initial_native_selection_uses_utf16_offsets() {
        let text = "😀 needs";

        assert_eq!(word_range_at_utf16(text, 4), Some(3..8));
    }

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

    actions!(
        selection_test,
        [SelectionMenuAction, OtherSelectionMenuAction]
    );

    struct SelectionActionTestView;

    impl Render for SelectionActionTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            selection_area(div().child(StyledText::new("hello").selectable()))
                .action_with_system_image("Add to Chat", "plus.bubble", SelectionMenuAction)
                .action("Other", OtherSelectionMenuAction)
        }
    }

    struct OccludedSelectionTestView;

    impl Render for OccludedSelectionTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .child(selection_area(
                    div().child(StyledText::new("hello").selectable()),
                ))
                .child(div().absolute().inset_0().occlude())
        }
    }

    struct PressableSelectionTestView {
        header_presses: Rc<Cell<usize>>,
        selection_presses: Rc<Cell<usize>>,
    }

    impl Render for PressableSelectionTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let header_presses = self.header_presses.clone();
            let selection_presses = self.selection_presses.clone();

            div()
                .w(px(240.0))
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("press-header")
                        .w(px(240.0))
                        .h(px(40.0))
                        .on_press(move |_, _, _| {
                            header_presses.set(header_presses.get() + 1);
                        })
                        .child("Header"),
                )
                .child(selection_area(
                    div()
                        .id("press-selection-body")
                        .w(px(240.0))
                        .h(px(80.0))
                        .on_press(move |_, _, _| {
                            selection_presses.set(selection_presses.get() + 1);
                        })
                        .child(StyledText::new("selectable").selectable()),
                ))
        }
    }

    struct LinkSelectionTestView {
        link_presses: Rc<Cell<usize>>,
        layout: Rc<RefCell<Option<TextLayout>>>,
        link_hit_slop: Option<Pixels>,
    }

    impl Render for LinkSelectionTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let link_presses = self.link_presses.clone();
            let text = StyledText::new("open link")
                .selectable()
                .selection_separator_after("\n");
            self.layout.borrow_mut().replace(text.layout().clone());
            let mut link = InteractiveText::new("selection-link", text).on_click(
                vec![5..9],
                move |_, _, _| {
                    link_presses.set(link_presses.get() + 1);
                },
            );
            if let Some(hit_slop) = self.link_hit_slop {
                link = link.hit_slop(hit_slop);
            }

            selection_area(div().w(px(240.0)).child(link))
        }
    }

    struct PressableOverlaySelectionTestView {
        overlay_presses: Rc<Cell<usize>>,
        selection_presses: Rc<Cell<usize>>,
    }

    impl Render for PressableOverlaySelectionTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let overlay_presses = self.overlay_presses.clone();
            let selection_presses = self.selection_presses.clone();

            div()
                .relative()
                .w(px(240.0))
                .h(px(80.0))
                .child(selection_area(
                    div()
                        .id("press-selection-underlay")
                        .size_full()
                        .on_press(move |_, _, _| {
                            selection_presses.set(selection_presses.get() + 1);
                        })
                        .child(StyledText::new("selectable").selectable()),
                ))
                .child(
                    div()
                        .id("press-overlay")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .on_press(move |_, _, _| {
                            overlay_presses.set(overlay_presses.get() + 1);
                        })
                        .child("Overlay"),
                )
        }
    }

    struct NormalOverlaySelectionTestView {
        overlay_presses: Rc<Cell<usize>>,
    }

    impl Render for NormalOverlaySelectionTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let overlay_presses = self.overlay_presses.clone();

            div()
                .relative()
                .w(px(240.0))
                .h(px(80.0))
                .child(selection_area(
                    div()
                        .size_full()
                        .child(StyledText::new("selectable").selectable()),
                ))
                .child(
                    div()
                        .id("normal-overlay")
                        .absolute()
                        .inset_0()
                        .on_press(move |_, _, _| {
                            overlay_presses.set(overlay_presses.get() + 1);
                        })
                        .child("Overlay"),
                )
        }
    }

    struct EmptySpaceSelectionAreaTestView;

    impl Render for EmptySpaceSelectionAreaTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            selection_area(
                div().w(px(240.0)).h(px(80.0)).child(
                    div()
                        .h(px(20.0))
                        .child(StyledText::new("selectable").selectable()),
                ),
            )
        }
    }

    struct MultilineEmptySpaceSelectionAreaTestView;

    impl Render for MultilineEmptySpaceSelectionAreaTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            selection_area(
                div()
                    .w(px(320.0))
                    .h(px(120.0))
                    .flex()
                    .flex_col()
                    .child(
                        div().h(px(20.0)).child(
                            StyledText::new("a")
                                .selectable()
                                .selection_order(0)
                                .selection_separator_after("\n"),
                        ),
                    )
                    .child(div().h(px(32.0)))
                    .child(
                        div()
                            .h(px(20.0))
                            .child(StyledText::new("bbbbbbbbbbbbbbbb").selectable()),
                    ),
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

        handler.set_selected_text_range(0..11);
        let latest = window
            .update(cx, |_, window, _| window.latest_read_only_selection())
            .unwrap()
            .unwrap();
        assert_eq!(latest.range_utf16, 0..11);
        assert_eq!(latest.text, "hello world");

        handler.clear_selected_text_range();
        window
            .update(cx, |_, window, _| {
                assert!(window.active_read_only_selection().is_none());
                assert_eq!(
                    window
                        .latest_read_only_selection()
                        .map(|selection| selection.text),
                    Some("hello world".to_string())
                );
                window.clear_read_only_selection_cache();
                assert!(window.latest_read_only_selection().is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn selection_handler_exposes_selection_area_actions(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| SelectionActionTestView)
            })
            .unwrap()
        });
        cx.run_until_parked();

        window
            .update(cx, |_, window, _| {
                let document = SelectionDocument::all(window).into_iter().next().unwrap();
                window.selection_state = SelectionState {
                    active_area: Some(document.key.clone()),
                    range_utf16: Some(0..5),
                    reversed: false,
                };
            })
            .unwrap();

        let test_window = cx.test_window(*window);
        let mut handler = test_window.take_selection_handler_for_test().unwrap();

        assert_eq!(
            handler.selection_action_names(),
            vec!["Add to Chat", "Other"]
        );
        let presentations = handler.selection_action_presentations();
        assert_eq!(presentations[0].name.to_string(), "Add to Chat");
        assert_eq!(
            presentations[0]
                .image_name
                .as_ref()
                .map(ToString::to_string),
            Some("plus.bubble".to_string())
        );
        assert_eq!(presentations[1].name.to_string(), "Other");
        assert!(presentations[1].image_name.is_none());

        handler.clear_selected_text_range();
        assert!(handler.selection_action_names().is_empty());
    }

    #[gpui::test]
    fn selection_handler_dispatches_selection_area_actions(cx: &mut TestAppContext) {
        let first_action_count = Rc::new(Cell::new(0));
        let second_action_count = Rc::new(Cell::new(0));
        cx.update(|cx| {
            let first_action_count = first_action_count.clone();
            let second_action_count = second_action_count.clone();
            cx.on_action(move |_: &SelectionMenuAction, _| {
                first_action_count.set(first_action_count.get() + 1);
            });
            cx.on_action(move |_: &OtherSelectionMenuAction, _| {
                second_action_count.set(second_action_count.get() + 1);
            });
        });

        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| SelectionActionTestView)
            })
            .unwrap()
        });
        cx.run_until_parked();

        window
            .update(cx, |_, window, _| {
                let document = SelectionDocument::all(window).into_iter().next().unwrap();
                window.selection_state = SelectionState {
                    active_area: Some(document.key.clone()),
                    range_utf16: Some(0..5),
                    reversed: false,
                };
            })
            .unwrap();

        let test_window = cx.test_window(*window);
        let mut handler = test_window.take_selection_handler_for_test().unwrap();

        assert_eq!(
            handler.selection_action_names(),
            vec!["Add to Chat", "Other"]
        );
        handler.perform_selection_action(1);
        cx.run_until_parked();
        assert_eq!(first_action_count.get(), 0);
        assert_eq!(second_action_count.get(), 1);

        handler.perform_selection_action(99);
        cx.run_until_parked();
        assert_eq!(first_action_count.get(), 0);
        assert_eq!(second_action_count.get(), 1);
    }

    #[gpui::test]
    fn selection_handler_hides_actions_for_empty_clamped_selection(cx: &mut TestAppContext) {
        let first_action_count = Rc::new(Cell::new(0));
        let second_action_count = Rc::new(Cell::new(0));
        cx.update(|cx| {
            let first_action_count = first_action_count.clone();
            let second_action_count = second_action_count.clone();
            cx.on_action(move |_: &SelectionMenuAction, _| {
                first_action_count.set(first_action_count.get() + 1);
            });
            cx.on_action(move |_: &OtherSelectionMenuAction, _| {
                second_action_count.set(second_action_count.get() + 1);
            });
        });

        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| SelectionActionTestView)
            })
            .unwrap()
        });
        cx.run_until_parked();

        window
            .update(cx, |_, window, _| {
                let document = SelectionDocument::all(window).into_iter().next().unwrap();
                window.selection_state = SelectionState {
                    active_area: Some(document.key.clone()),
                    range_utf16: Some(10..20),
                    reversed: false,
                };
            })
            .unwrap();

        let test_window = cx.test_window(*window);
        let mut handler = test_window.take_selection_handler_for_test().unwrap();

        assert!(handler.selection_action_names().is_empty());
        handler.perform_selection_action(0);
        cx.run_until_parked();
        assert_eq!(first_action_count.get(), 0);
        assert_eq!(second_action_count.get(), 0);
    }

    #[gpui::test]
    fn selection_handler_preserves_selection_when_global_key_shifts(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(|_| SelectionTestView))
                .unwrap()
        });
        cx.run_until_parked();

        window
            .update(cx, |_, window, _| {
                let document = SelectionDocument::all(window).into_iter().next().unwrap();
                let area_id = document.key.selection_area_id().unwrap().clone();
                window.selection_state = SelectionState {
                    active_area: Some(SelectionAreaKey::ViewScoped(EntityId::from(9_999), area_id)),
                    range_utf16: Some(0..5),
                    reversed: false,
                };
            })
            .unwrap();

        let test_window = cx.test_window(*window);
        let mut handler = test_window.take_selection_handler_for_test().unwrap();

        assert_eq!(handler.selected_text_range(false).unwrap().range, 0..5);
        assert!(handler.bounds_for_range(0..5).is_some());
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
    fn selection_handler_ignores_points_occluded_after_selection_area(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| OccludedSelectionTestView)
            })
            .unwrap()
        });
        cx.run_until_parked();

        let hit_point = window
            .update(cx, |_, window, _| {
                let document = SelectionDocument::all(window).into_iter().next().unwrap();
                document.bounds_for_range(0..1).unwrap().center()
            })
            .unwrap();

        let test_window = cx.test_window(*window);
        let mut handler = test_window.take_selection_handler_for_test().unwrap();

        assert_eq!(handler.character_index_for_point(hit_point), None);
    }

    #[gpui::test]
    fn selection_handler_allows_on_press_outside_selection_area(cx: &mut TestAppContext) {
        let header_presses = Rc::new(Cell::new(0));
        let selection_presses = Rc::new(Cell::new(0));
        let window = open_pressable_selection_test_window(
            cx,
            header_presses.clone(),
            selection_presses.clone(),
        );
        let header_point = point(px(20.0), px(20.0));

        assert!(!selection_fast_hit_cache_claims_point(
            cx,
            window,
            header_point
        ));
        let selection_claimed_touch = selection_handler_claims_point(cx, window, header_point);
        assert!(!selection_claimed_touch);
        if !selection_claimed_touch {
            simulate_primary_click(cx, window, header_point);
        }

        assert_eq!(header_presses.get(), 1);
        assert_eq!(selection_presses.get(), 0);
    }

    #[gpui::test]
    fn selection_handler_claims_selectable_text_before_on_press(cx: &mut TestAppContext) {
        let header_presses = Rc::new(Cell::new(0));
        let selection_presses = Rc::new(Cell::new(0));
        let window = open_pressable_selection_test_window(
            cx,
            header_presses.clone(),
            selection_presses.clone(),
        );
        let selectable_text_point = cx
            .update_window(window, |_, window, _| {
                let document = SelectionDocument::all(window).into_iter().next().unwrap();
                document.bounds_for_range(0..1).unwrap().center()
            })
            .unwrap();

        assert!(selection_fast_hit_cache_claims_point(
            cx,
            window,
            selectable_text_point
        ));
        let selection_claimed_touch =
            selection_handler_claims_point(cx, window, selectable_text_point);
        assert!(selection_claimed_touch);
        if !selection_claimed_touch {
            simulate_primary_click(cx, window, selectable_text_point);
        }

        assert_eq!(header_presses.get(), 0);
        assert_eq!(selection_presses.get(), 0);
    }

    #[gpui::test]
    fn interactive_text_link_hitboxes_take_precedence_over_selection(cx: &mut TestAppContext) {
        let link_presses = Rc::new(Cell::new(0));
        let layout = Rc::new(RefCell::new(None));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| LinkSelectionTestView {
                    link_presses: link_presses.clone(),
                    layout: layout.clone(),
                    link_hit_slop: None,
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        let window = *window;
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear();
        })
        .unwrap();
        let link_point = layout
            .borrow()
            .as_ref()
            .unwrap()
            .rects_for_range(5..9)
            .first()
            .unwrap()
            .center();

        assert!(!selection_fast_hit_cache_claims_point(
            cx, window, link_point
        ));
        let selection_claimed_touch = selection_handler_claims_point(cx, window, link_point);
        assert!(!selection_claimed_touch);
        simulate_primary_click(cx, window, link_point);

        assert_eq!(link_presses.get(), 1);
    }

    #[gpui::test]
    fn interactive_text_link_hit_slop_takes_precedence_over_selection(cx: &mut TestAppContext) {
        let link_presses = Rc::new(Cell::new(0));
        let layout = Rc::new(RefCell::new(None));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| LinkSelectionTestView {
                    link_presses: link_presses.clone(),
                    layout: layout.clone(),
                    link_hit_slop: Some(px(8.0)),
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        let window = *window;
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear();
        })
        .unwrap();
        let link_bounds = *layout
            .borrow()
            .as_ref()
            .unwrap()
            .rects_for_range(5..9)
            .first()
            .unwrap();
        let slop_point = point(link_bounds.right() + px(4.0), link_bounds.center().y);

        assert!(!selection_fast_hit_cache_claims_point(
            cx, window, slop_point
        ));
        simulate_primary_click(cx, window, slop_point);

        assert_eq!(link_presses.get(), 1);
    }

    #[gpui::test]
    fn selection_area_cache_allows_nearest_position_without_starting_on_empty_space(
        cx: &mut TestAppContext,
    ) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| EmptySpaceSelectionAreaTestView)
            })
            .unwrap()
        });
        cx.run_until_parked();
        let window = *window;
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear();
        })
        .unwrap();
        let empty_selection_area_point = point(px(20.0), px(60.0));

        assert!(!selection_fast_hit_cache_claims_point(
            cx,
            window,
            empty_selection_area_point
        ));
        assert!(selection_fast_hit_cache_claims_selection_area(
            cx,
            window,
            empty_selection_area_point
        ));

        let mut handler = cx
            .test_window(window)
            .take_selection_handler_for_test()
            .unwrap();
        assert_eq!(
            handler.character_index_for_point(empty_selection_area_point),
            None
        );
        assert!(
            handler
                .nearest_character_index_for_point(empty_selection_area_point)
                .is_some()
        );
    }

    #[gpui::test]
    fn nearest_selection_position_prefers_nearest_visual_row(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| MultilineEmptySpaceSelectionAreaTestView)
            })
            .unwrap()
        });
        cx.run_until_parked();
        let window = *window;
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear();
        })
        .unwrap();
        let empty_space_after_first_line = cx
            .update_window(window, |_, window, _| {
                let document = SelectionDocument::all(window).into_iter().next().unwrap();
                let first_line_bounds = document
                    .fragments
                    .first()
                    .unwrap()
                    .fragment
                    .fragment()
                    .layout()
                    .bounds();
                let selection_bounds = document.bounds.unwrap();
                point(
                    selection_bounds.right() - px(4.0),
                    first_line_bounds.bottom() + px(4.0),
                )
            })
            .unwrap();

        let mut handler = cx
            .test_window(window)
            .take_selection_handler_for_test()
            .unwrap();

        assert_eq!(
            handler.character_index_for_point(empty_space_after_first_line),
            None
        );
        assert_eq!(
            handler.nearest_character_index_for_point(empty_space_after_first_line),
            Some(1)
        );
    }

    #[gpui::test]
    fn focus_change_clears_active_selection(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(|_| SelectionTestView))
                .unwrap()
        });
        cx.run_until_parked();

        window
            .update(cx, |_, window, cx| {
                let document = SelectionDocument::all(window).into_iter().next().unwrap();
                window.selection_state = SelectionState {
                    active_area: Some(document.key.clone()),
                    range_utf16: Some(0..5),
                    reversed: false,
                };

                let focus_handle = cx.focus_handle();
                window.focus(&focus_handle, cx);

                assert!(window.selection_state.active_area.is_none());
                assert!(window.selection_state.range_utf16.is_none());
                assert!(!window.selection_state.reversed);
            })
            .unwrap();
    }

    #[gpui::test]
    fn selection_handler_allows_on_press_on_overlay_above_selection_area(cx: &mut TestAppContext) {
        let overlay_presses = Rc::new(Cell::new(0));
        let selection_presses = Rc::new(Cell::new(0));
        let window = open_pressable_overlay_selection_test_window(
            cx,
            overlay_presses.clone(),
            selection_presses.clone(),
        );
        let overlay_point = cx
            .update_window(window, |_, window, _| {
                let document = SelectionDocument::all(window).into_iter().next().unwrap();
                document.bounds_for_range(0..1).unwrap().center()
            })
            .unwrap();

        assert!(!selection_fast_hit_cache_claims_point(
            cx,
            window,
            overlay_point
        ));
        let selection_claimed_touch = selection_handler_claims_point(cx, window, overlay_point);
        assert!(!selection_claimed_touch);
        if !selection_claimed_touch {
            simulate_primary_click(cx, window, overlay_point);
        }

        assert_eq!(overlay_presses.get(), 1);
        assert_eq!(selection_presses.get(), 0);
    }

    #[gpui::test]
    fn selection_handler_ignores_normal_overlay_above_selection_area(cx: &mut TestAppContext) {
        let overlay_presses = Rc::new(Cell::new(0));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| NormalOverlaySelectionTestView {
                    overlay_presses: overlay_presses.clone(),
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        let window = *window;
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear();
        })
        .unwrap();
        let selection_point = cx
            .update_window(window, |_, window, _| {
                let document = SelectionDocument::all(window).into_iter().next().unwrap();
                document.bounds_for_range(0..1).unwrap().center()
            })
            .unwrap();

        assert!(selection_fast_hit_cache_claims_point(
            cx,
            window,
            selection_point
        ));
        assert!(selection_handler_claims_point(cx, window, selection_point));
        assert_eq!(overlay_presses.get(), 0);
    }

    fn open_pressable_selection_test_window(
        cx: &mut TestAppContext,
        header_presses: Rc<Cell<usize>>,
        selection_presses: Rc<Cell<usize>>,
    ) -> AnyWindowHandle {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| PressableSelectionTestView {
                    header_presses,
                    selection_presses,
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        let window = *window;
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear();
        })
        .unwrap();
        window
    }

    fn open_pressable_overlay_selection_test_window(
        cx: &mut TestAppContext,
        overlay_presses: Rc<Cell<usize>>,
        selection_presses: Rc<Cell<usize>>,
    ) -> AnyWindowHandle {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| PressableOverlaySelectionTestView {
                    overlay_presses,
                    selection_presses,
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        let window = *window;
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear();
        })
        .unwrap();
        window
    }

    fn selection_handler_claims_point(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        point: crate::Point<crate::Pixels>,
    ) -> bool {
        let mut handler = cx
            .test_window(window)
            .take_selection_handler_for_test()
            .unwrap();
        handler.character_index_for_point(point).is_some()
    }

    fn selection_fast_hit_cache_claims_point(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        point: crate::Point<crate::Pixels>,
    ) -> bool {
        cx.test_window(window)
            .selectable_text_hit_regions_for_test()
            .iter()
            .any(|region| region.contains_text(point))
    }

    fn selection_fast_hit_cache_claims_selection_area(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        point: crate::Point<crate::Pixels>,
    ) -> bool {
        cx.test_window(window)
            .selectable_text_hit_regions_for_test()
            .iter()
            .any(|region| region.contains_selection_area(point))
    }

    fn simulate_primary_click(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        point: crate::Point<crate::Pixels>,
    ) {
        let mut test_window = cx.test_window(window);
        test_window.simulate_input(
            PointerDownEvent {
                pointer_id: 1,
                kind: PointerKind::Touch,
                is_primary: true,
                position: point,
                button: PointerButton::Primary,
                modifiers: Modifiers::default(),
            }
            .to_platform_input(),
        );
        test_window.simulate_input(
            PointerUpEvent {
                pointer_id: 1,
                kind: PointerKind::Touch,
                is_primary: true,
                position: point,
                button: PointerButton::Primary,
                modifiers: Modifiers::default(),
            }
            .to_platform_input(),
        );
        cx.run_until_parked();
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
