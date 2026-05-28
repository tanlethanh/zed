use std::panic::Location;

use crate::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, ScreenTransform, Window,
};

/// Wrap `child` so every primitive it emits (and every hitbox it inserts) is
/// scaled and translated by `transform` into screen space at paint and
/// prepaint time. Combined with [`ScreenTransform::scale_around_pivot`] this
/// is the building block for subtree canvas zoom — layout stays at natural
/// size, but the resulting geometry is composited at a different scale.
///
/// The wrapper inherits its layout from the child, so use it inside a
/// container that constrains size; the visual extent past zoom > 1.0 will
/// overflow that container and should be clipped by the caller via
/// `.overflow_hidden()` if undesired.
#[track_caller]
pub fn paint_transform<E: IntoElement>(
    transform: ScreenTransform,
    child: E,
) -> PaintTransform<E::Element> {
    PaintTransform {
        transform,
        child: child.into_element(),
        source_location: Location::caller(),
    }
}

/// The element returned by [`paint_transform`].
pub struct PaintTransform<E> {
    transform: ScreenTransform,
    child: E,
    source_location: &'static Location<'static>,
}

impl<E: Element> IntoElement for PaintTransform<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: Element> Element for PaintTransform<E> {
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.child.id()
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        Some(self.source_location)
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // Layout runs in the subtree's untransformed coordinate space so
        // wrapping width, line breaks and intrinsic sizes do not change as
        // the user zooms. The transform only affects paint and hit-test.
        self.child.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let transform = self.transform;
        window.with_paint_transform(transform, |window| {
            self.child
                .prepaint(id, inspector_id, bounds, state, window, cx)
        })
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let transform = self.transform;
        window.with_paint_transform(transform, |window| {
            self.child
                .paint(id, inspector_id, bounds, request_layout, prepaint, window, cx);
        });
    }
}
