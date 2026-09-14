//! Pure touch and pinch gesture arbitration state machine.

use crate::{
    Modifiers, PinchEvent, Pixels, PlatformInput, Point, PointerButton, PointerCancelEvent,
    PointerDownEvent, PointerKind, PointerMoveEvent, PointerUpEvent, ScrollDelta, ScrollWheelEvent,
    TouchPhase,
};
use smallvec::SmallVec;

/// State of an active pinch gesture relative to claim resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinchState {
    /// No pinch gesture is active.
    Idle,
    /// A pinch gesture began and is waiting for GPUI claim resolution.
    StartedPendingClaim,
    /// The pinch gesture was claimed by GPUI.
    Claimed,
    /// The pinch gesture was not claimed and raw touch streams continue.
    Unclaimed,
}

/// Events produced by the touch and pinch arbitrator.
#[derive(Clone, Debug)]
pub enum ArbitratedEvent {
    /// A platform input event to be dispatched to GPUI.
    PlatformInput(PlatformInput),
    /// Fling should be cancelled immediately.
    CancelFling,
}

impl ArbitratedEvent {
    /// Creates a PointerDown event.
    pub fn pointer_down(pointer_id: u64, position: Point<Pixels>, modifiers: Modifiers) -> Self {
        ArbitratedEvent::PlatformInput(PlatformInput::PointerDown(PointerDownEvent {
            pointer_id,
            kind: PointerKind::Touch,
            is_primary: true,
            button: PointerButton::Primary,
            position,
            modifiers,
        }))
    }

    /// Creates a PointerMove event.
    pub fn pointer_move(pointer_id: u64, position: Point<Pixels>, modifiers: Modifiers) -> Self {
        ArbitratedEvent::PlatformInput(PlatformInput::PointerMove(PointerMoveEvent {
            pointer_id,
            kind: PointerKind::Touch,
            is_primary: true,
            pressed_button: Some(PointerButton::Primary),
            position,
            modifiers,
        }))
    }

    /// Creates a PointerUp event.
    pub fn pointer_up(pointer_id: u64, position: Point<Pixels>, modifiers: Modifiers) -> Self {
        ArbitratedEvent::PlatformInput(PlatformInput::PointerUp(PointerUpEvent {
            pointer_id,
            kind: PointerKind::Touch,
            is_primary: true,
            button: PointerButton::Primary,
            position,
            modifiers,
        }))
    }

    /// Creates a PointerCancel event.
    pub fn pointer_cancel(pointer_id: u64, position: Point<Pixels>, modifiers: Modifiers) -> Self {
        ArbitratedEvent::PlatformInput(PlatformInput::PointerCancel(PointerCancelEvent {
            pointer_id,
            kind: PointerKind::Touch,
            is_primary: true,
            position,
            modifiers,
        }))
    }

    /// Creates a ScrollWheel event.
    pub fn scroll(
        position: Point<Pixels>,
        delta: Point<Pixels>,
        modifiers: Modifiers,
        touch_phase: TouchPhase,
    ) -> Self {
        ArbitratedEvent::PlatformInput(PlatformInput::ScrollWheel(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(delta),
            modifiers,
            touch_phase,
        }))
    }

    /// Creates a Pinch event.
    pub fn pinch(pinch: PinchEvent) -> Self {
        ArbitratedEvent::PlatformInput(PlatformInput::Pinch(pinch))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct PrimaryTouchState {
    id: u64,
    down_position: Point<Pixels>,
    current_position: Point<Pixels>,
    pointer_down_emitted: bool,
    scroll_started: bool,
}

/// Arbitrates between raw multi-touch streams and UIKit pinch gestures.
#[derive(Clone, Debug, PartialEq)]
pub struct TouchPinchArbitrator {
    active_touches: SmallVec<[u64; 8]>,
    quarantined_touches: SmallVec<[u64; 8]>,
    primary_touch: Option<PrimaryTouchState>,
    pinch_state: PinchState,
    last_center: Point<Pixels>,
    last_scale: f32,
}

impl Default for TouchPinchArbitrator {
    fn default() -> Self {
        Self::new()
    }
}

impl TouchPinchArbitrator {
    /// Creates a new TouchPinchArbitrator in idle state.
    pub fn new() -> Self {
        Self {
            active_touches: SmallVec::new(),
            quarantined_touches: SmallVec::new(),
            primary_touch: None,
            pinch_state: PinchState::Idle,
            last_center: Point::default(),
            last_scale: 1.0,
        }
    }

    /// Returns true if a pinch gesture has been claimed.
    pub fn is_pinch_claimed(&self) -> bool {
        self.pinch_state == PinchState::Claimed
    }

    /// Returns true if the touch pointer is currently quarantined.
    pub fn is_touch_quarantined(&self, touch_id: u64) -> bool {
        self.quarantined_touches.contains(&touch_id)
    }

    /// Handles a touch began event from the platform.
    pub fn touch_began(
        &mut self,
        touch_id: u64,
        position: Point<Pixels>,
        modifiers: Modifiers,
    ) -> SmallVec<[ArbitratedEvent; 4]> {
        if !self.active_touches.contains(&touch_id) {
            self.active_touches.push(touch_id);
        }

        if self.pinch_state == PinchState::Claimed {
            if !self.quarantined_touches.contains(&touch_id) {
                self.quarantined_touches.push(touch_id);
            }
            return SmallVec::new();
        }

        if self.quarantined_touches.contains(&touch_id) {
            return SmallVec::new();
        }

        let mut events = SmallVec::new();
        if self.primary_touch.is_none() && self.quarantined_touches.is_empty() {
            self.primary_touch = Some(PrimaryTouchState {
                id: touch_id,
                down_position: position,
                current_position: position,
                pointer_down_emitted: true,
                scroll_started: false,
            });
            events.push(ArbitratedEvent::CancelFling);
            events.push(ArbitratedEvent::pointer_down(touch_id, position, modifiers));
        } else {
            events.push(ArbitratedEvent::CancelFling);
        }

        events
    }

    /// Handles a touch moved event from the platform.
    pub fn touch_moved(
        &mut self,
        touch_id: u64,
        position: Point<Pixels>,
        delta: Point<Pixels>,
        modifiers: Modifiers,
    ) -> SmallVec<[ArbitratedEvent; 4]> {
        if self.quarantined_touches.contains(&touch_id) {
            return SmallVec::new();
        }

        if self.pinch_state == PinchState::Claimed {
            return SmallVec::new();
        }

        let mut events = SmallVec::new();
        if let Some(primary) = &mut self.primary_touch {
            if primary.id == touch_id {
                primary.current_position = position;
                primary.scroll_started = true;
                events.push(ArbitratedEvent::pointer_move(touch_id, position, modifiers));
                events.push(ArbitratedEvent::scroll(
                    primary.down_position,
                    delta,
                    modifiers,
                    TouchPhase::Moved,
                ));
            }
        }

        events
    }

    /// Handles a touch ended event from the platform.
    pub fn touch_ended(
        &mut self,
        touch_id: u64,
        position: Point<Pixels>,
        modifiers: Modifiers,
    ) -> SmallVec<[ArbitratedEvent; 4]> {
        self.active_touches.retain(|id| *id != touch_id);

        if let Some(index) = self
            .quarantined_touches
            .iter()
            .position(|id| *id == touch_id)
        {
            self.quarantined_touches.swap_remove(index);
            if self.active_touches.is_empty() {
                self.quarantined_touches.clear();
                self.primary_touch = None;
            }
            return SmallVec::new();
        }

        if self.pinch_state == PinchState::Claimed {
            return SmallVec::new();
        }

        let mut events = SmallVec::new();
        if let Some(primary) = self.primary_touch.take() {
            if primary.id == touch_id {
                if primary.pointer_down_emitted {
                    events.push(ArbitratedEvent::pointer_up(touch_id, position, modifiers));
                }
            } else {
                self.primary_touch = Some(primary);
            }
        }

        events
    }

    /// Handles a touch cancelled event from the platform.
    pub fn touch_cancelled(
        &mut self,
        touch_id: u64,
        position: Point<Pixels>,
        modifiers: Modifiers,
    ) -> SmallVec<[ArbitratedEvent; 4]> {
        self.active_touches.retain(|id| *id != touch_id);

        if let Some(index) = self
            .quarantined_touches
            .iter()
            .position(|id| *id == touch_id)
        {
            self.quarantined_touches.swap_remove(index);
            if self.active_touches.is_empty() {
                self.quarantined_touches.clear();
                self.primary_touch = None;
            }
            return SmallVec::new();
        }

        if self.pinch_state == PinchState::Claimed {
            return SmallVec::new();
        }

        let mut events = SmallVec::new();
        if let Some(primary) = self.primary_touch.take() {
            if primary.id == touch_id {
                if primary.pointer_down_emitted {
                    events.push(ArbitratedEvent::pointer_cancel(
                        touch_id, position, modifiers,
                    ));
                }
            } else {
                self.primary_touch = Some(primary);
            }
        }

        events
    }

    /// Processes the began phase of a pinch gesture recognizer.
    pub fn pinch_began(
        &mut self,
        center: Point<Pixels>,
        scale: f32,
        modifiers: Modifiers,
    ) -> Option<PinchEvent> {
        let valid_scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };

        self.last_scale = valid_scale;
        self.last_center = center;
        self.pinch_state = PinchState::StartedPendingClaim;

        Some(PinchEvent {
            position: center,
            delta: 0.0,
            modifiers,
            phase: TouchPhase::Started,
        })
    }

    /// Resolves whether the pinch gesture was claimed by GPUI.
    pub fn resolve_pinch_claim(
        &mut self,
        is_claimed: bool,
        modifiers: Modifiers,
    ) -> SmallVec<[ArbitratedEvent; 4]> {
        if self.pinch_state != PinchState::StartedPendingClaim {
            return SmallVec::new();
        }

        if is_claimed {
            self.pinch_state = PinchState::Claimed;

            for touch_id in &self.active_touches {
                if !self.quarantined_touches.contains(touch_id) {
                    self.quarantined_touches.push(*touch_id);
                }
            }

            let mut events = SmallVec::new();
            events.push(ArbitratedEvent::CancelFling);

            if let Some(primary) = self.primary_touch.take() {
                if primary.pointer_down_emitted {
                    events.push(ArbitratedEvent::pointer_cancel(
                        primary.id,
                        primary.current_position,
                        modifiers,
                    ));
                }
                if primary.scroll_started {
                    events.push(ArbitratedEvent::scroll(
                        primary.down_position,
                        Point::default(),
                        modifiers,
                        TouchPhase::Ended,
                    ));
                }
            }

            events
        } else {
            self.pinch_state = PinchState::Unclaimed;
            SmallVec::new()
        }
    }

    /// Processes the changed phase of a pinch gesture recognizer.
    pub fn pinch_changed(
        &mut self,
        center: Point<Pixels>,
        scale: f32,
        modifiers: Modifiers,
    ) -> Option<PinchEvent> {
        if self.pinch_state != PinchState::Claimed {
            return None;
        }

        let delta = if scale.is_finite() && scale > 0.0 && self.last_scale > 0.0 {
            let incremental = (scale - self.last_scale) / self.last_scale;
            if incremental.is_finite() {
                self.last_scale = scale;
                incremental
            } else {
                0.0
            }
        } else {
            0.0
        };

        self.last_center = center;

        Some(PinchEvent {
            position: center,
            delta,
            modifiers,
            phase: TouchPhase::Moved,
        })
    }

    /// Processes the ended phase of a pinch gesture recognizer.
    pub fn pinch_ended(&mut self, modifiers: Modifiers) -> Option<PinchEvent> {
        if self.pinch_state != PinchState::Claimed {
            self.pinch_state = PinchState::Idle;
            return None;
        }

        self.pinch_state = PinchState::Idle;

        Some(PinchEvent {
            position: self.last_center,
            delta: 0.0,
            modifiers,
            phase: TouchPhase::Ended,
        })
    }

    /// Processes the cancelled phase of a pinch gesture recognizer.
    pub fn pinch_cancelled(&mut self, modifiers: Modifiers) -> Option<PinchEvent> {
        self.pinch_ended(modifiers)
    }
}
