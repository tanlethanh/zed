//! iOS event handling - converting UIKit events to GPUI's event types.
//!
//! iOS uses touch-based input rather than mouse input, so we map touches to
//! GPUI pointer and scroll events directly:
//! - Single tap → PointerDown + PointerUp
//! - Pan gesture → ScrollWheel events
//! - Touch move → PointerMove events

use core_graphics::geometry::CGPoint;
use gpui::{
    Modifiers, Pixels, PlatformInput, Point, PointerButton, PointerCancelEvent, PointerDownEvent,
    PointerKind, PointerMoveEvent, PointerUpEvent, ScrollDelta, ScrollWheelEvent, TouchPhase, px,
};
use objc::{msg_send, runtime::Object, sel, sel_impl};

/// Touch phase from UIKit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum UITouchPhase {
    Began = 0,
    Moved = 1,
    Stationary = 2,
    Ended = 3,
    Cancelled = 4,
}

impl From<i64> for UITouchPhase {
    fn from(value: i64) -> Self {
        match value {
            0 => UITouchPhase::Began,
            1 => UITouchPhase::Moved,
            2 => UITouchPhase::Stationary,
            3 => UITouchPhase::Ended,
            4 => UITouchPhase::Cancelled,
            _ => UITouchPhase::Cancelled,
        }
    }
}

impl From<UITouchPhase> for TouchPhase {
    fn from(phase: UITouchPhase) -> Self {
        match phase {
            UITouchPhase::Began => TouchPhase::Started,
            UITouchPhase::Moved => TouchPhase::Moved,
            UITouchPhase::Stationary => TouchPhase::Moved,
            UITouchPhase::Ended => TouchPhase::Ended,
            UITouchPhase::Cancelled => TouchPhase::Ended,
        }
    }
}

/// Convert a UITouch to a mouse position
pub fn touch_location_in_view(touch: *mut Object, view: *mut Object) -> Point<Pixels> {
    unsafe {
        let location: CGPoint = msg_send![touch, locationInView: view];
        Point::new(px(location.x as f32), px(location.y as f32))
    }
}

/// Get the touch phase from a UITouch
pub fn touch_phase(touch: *mut Object) -> UITouchPhase {
    unsafe {
        let phase: i64 = msg_send![touch, phase];
        UITouchPhase::from(phase)
    }
}

/// Get the previous location of a UITouch in the given view (for computing scroll delta)
pub fn touch_previous_location_in_view(touch: *mut Object, view: *mut Object) -> Point<Pixels> {
    unsafe {
        let location: CGPoint = msg_send![touch, previousLocationInView: view];
        Point::new(px(location.x as f32), px(location.y as f32))
    }
}

/// Convert a single touch began event to a pointer down event.
pub fn touch_began_to_pointer_down(
    position: Point<Pixels>,
    pointer_id: u64,
    modifiers: Modifiers,
) -> PlatformInput {
    PlatformInput::PointerDown(PointerDownEvent {
        pointer_id,
        kind: PointerKind::Touch,
        is_primary: true,
        button: PointerButton::Primary,
        position,
        modifiers,
    })
}

/// Convert a touch ended event to a pointer up event.
pub fn touch_ended_to_pointer_up(
    position: Point<Pixels>,
    pointer_id: u64,
    modifiers: Modifiers,
) -> PlatformInput {
    PlatformInput::PointerUp(PointerUpEvent {
        pointer_id,
        kind: PointerKind::Touch,
        is_primary: true,
        button: PointerButton::Primary,
        position,
        modifiers,
    })
}

/// Convert a touch moved event to a pointer move event.
pub fn touch_moved_to_pointer_move(
    position: Point<Pixels>,
    pointer_id: u64,
    modifiers: Modifiers,
) -> PlatformInput {
    PlatformInput::PointerMove(PointerMoveEvent {
        pointer_id,
        kind: PointerKind::Touch,
        is_primary: true,
        pressed_button: Some(PointerButton::Primary),
        position,
        modifiers,
    })
}

/// Convert a touch cancellation into a pointer cancel event.
pub fn touch_cancelled_to_pointer_cancel(
    position: Point<Pixels>,
    pointer_id: u64,
    modifiers: Modifiers,
) -> PlatformInput {
    PlatformInput::PointerCancel(PointerCancelEvent {
        pointer_id,
        kind: PointerKind::Touch,
        is_primary: true,
        position,
        modifiers,
    })
}

/// Convert a pan gesture to a scroll wheel event
pub fn pan_gesture_to_scroll(
    position: Point<Pixels>,
    delta: Point<Pixels>,
    modifiers: Modifiers,
    touch_phase: TouchPhase,
) -> PlatformInput {
    PlatformInput::ScrollWheel(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(delta),
        modifiers,
        touch_phase,
    })
}

/// Get current keyboard modifiers from UIKit.
/// iOS doesn't have the same modifier key concept as macOS,
/// but external keyboards can provide modifiers.
pub fn get_current_modifiers() -> Modifiers {
    // iOS 13.4+ supports modifier keys from external keyboards
    // For now, return empty modifiers - this can be enhanced later
    // to read from UIKeyModifierFlags when available
    Modifiers::default()
}
