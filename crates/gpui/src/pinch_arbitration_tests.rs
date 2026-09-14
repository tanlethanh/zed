#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LegacyTouchPhase {
    Began,
    Moved,
    Ended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LegacyTouch {
    id: u64,
    phase: LegacyTouchPhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LegacyObservation {
    PrimaryBegan(u64),
    PrimaryMoved(u64),
    PrimaryEnded(u64),
}

fn route_legacy_single_primary_touches(events: &[LegacyTouch]) -> Vec<LegacyObservation> {
    let mut primary_touch = None;
    let mut observations = Vec::new();

    for event in events {
        match event.phase {
            LegacyTouchPhase::Began if primary_touch.is_none() => {
                primary_touch = Some(event.id);
                observations.push(LegacyObservation::PrimaryBegan(event.id));
            }
            LegacyTouchPhase::Moved if primary_touch == Some(event.id) => {
                observations.push(LegacyObservation::PrimaryMoved(event.id));
            }
            LegacyTouchPhase::Ended if primary_touch == Some(event.id) => {
                observations.push(LegacyObservation::PrimaryEnded(event.id));
                primary_touch = None;
            }
            LegacyTouchPhase::Began | LegacyTouchPhase::Moved | LegacyTouchPhase::Ended => {}
        }
    }

    observations
}

#[test]
fn baseline_single_primary_ignores_secondary_touch_activity() {
    let events = [
        LegacyTouch {
            id: 1,
            phase: LegacyTouchPhase::Began,
        },
        LegacyTouch {
            id: 2,
            phase: LegacyTouchPhase::Began,
        },
        LegacyTouch {
            id: 2,
            phase: LegacyTouchPhase::Moved,
        },
        LegacyTouch {
            id: 2,
            phase: LegacyTouchPhase::Ended,
        },
        LegacyTouch {
            id: 1,
            phase: LegacyTouchPhase::Moved,
        },
        LegacyTouch {
            id: 1,
            phase: LegacyTouchPhase::Ended,
        },
    ];

    assert_eq!(
        route_legacy_single_primary_touches(&events),
        vec![
            LegacyObservation::PrimaryBegan(1),
            LegacyObservation::PrimaryMoved(1),
            LegacyObservation::PrimaryEnded(1),
        ]
    );
}

#[test]
fn pinch_arbitration_claimed_pinch_cancels_and_quarantines_survivor() {
    use crate::touch_arbitration::{ArbitratedEvent, TouchPinchArbitrator};
    use crate::{Modifiers, Point, TouchPhase, px};

    let mut arbitrator = TouchPinchArbitrator::new();
    let mod_none = Modifiers::default();

    let touch1_down = arbitrator.touch_began(1, Point::new(px(100.0), px(100.0)), mod_none);
    assert!(
        touch1_down.iter().any(|e| matches!(
            e,
            ArbitratedEvent::PlatformInput(crate::PlatformInput::PointerDown(_))
        )),
        "Expected pointer down for touch 1"
    );

    let touch1_move = arbitrator.touch_moved(
        1,
        Point::new(px(105.0), px(100.0)),
        Point::new(px(5.0), px(0.0)),
        mod_none,
    );
    assert!(
        touch1_move.iter().any(|e| matches!(
            e,
            ArbitratedEvent::PlatformInput(crate::PlatformInput::PointerMove(_))
        )),
        "Expected pointer move for touch 1"
    );

    let touch2_down = arbitrator.touch_began(2, Point::new(px(200.0), px(100.0)), mod_none);
    assert!(
        !touch2_down.iter().any(|e| matches!(
            e,
            ArbitratedEvent::PlatformInput(crate::PlatformInput::PointerDown(_))
        )),
        "Secondary touch should not emit pointer down"
    );

    let pinch_start = arbitrator.pinch_began(Point::new(px(150.0), px(100.0)), 1.0, mod_none);
    assert!(pinch_start.is_some());
    let pinch_start = pinch_start.unwrap();
    assert_eq!(pinch_start.phase, TouchPhase::Started);

    let claim_events = arbitrator.resolve_pinch_claim(true, mod_none);
    assert!(
        claim_events
            .iter()
            .any(|e| matches!(e, ArbitratedEvent::CancelFling)),
        "Claim must cancel fling"
    );
    assert!(
        claim_events.iter().any(|e| matches!(
            e,
            ArbitratedEvent::PlatformInput(crate::PlatformInput::PointerCancel(_))
        )),
        "Claim must cancel pointer down exactly once"
    );
    assert!(
        claim_events.iter().any(|e| matches!(
            e,
            ArbitratedEvent::PlatformInput(crate::PlatformInput::ScrollWheel(_))
        )),
        "Claim must end active scroll exactly once"
    );

    let pinch_move = arbitrator.pinch_changed(Point::new(px(150.0), px(100.0)), 1.1, mod_none);
    assert!(pinch_move.is_some());
    let pinch_move = pinch_move.unwrap();
    assert_eq!(pinch_move.phase, TouchPhase::Moved);
    assert!(
        (pinch_move.delta - 0.1).abs() < 1e-4,
        "Delta should be ~0.1, got {}",
        pinch_move.delta
    );

    let pinch_end = arbitrator.pinch_ended(mod_none);
    assert!(pinch_end.is_some());
    let pinch_end = pinch_end.unwrap();
    assert_eq!(pinch_end.phase, TouchPhase::Ended);
    assert_eq!(pinch_end.delta, 0.0);

    let touch2_end = arbitrator.touch_ended(2, Point::new(px(200.0), px(100.0)), mod_none);
    assert!(
        touch2_end.is_empty(),
        "Quarantined touch lift must be silent"
    );

    let touch1_survivor_move = arbitrator.touch_moved(
        1,
        Point::new(px(120.0), px(100.0)),
        Point::new(px(15.0), px(0.0)),
        mod_none,
    );
    assert!(
        touch1_survivor_move.is_empty(),
        "Surviving touch move must be quarantined"
    );

    let touch1_end = arbitrator.touch_ended(1, Point::new(px(120.0), px(100.0)), mod_none);
    assert!(
        touch1_end.is_empty(),
        "Surviving touch lift must be quarantined"
    );

    let touch3_down = arbitrator.touch_began(3, Point::new(px(50.0), px(50.0)), mod_none);
    assert!(
        touch3_down.iter().any(|e| matches!(
            e,
            ArbitratedEvent::PlatformInput(crate::PlatformInput::PointerDown(_))
        )),
        "Fresh touch 3 after quarantine reset must emit pointer down"
    );
}

#[test]
fn pinch_arbitration_unclaimed_pinch_leaves_touch_stream_intact() {
    use crate::touch_arbitration::{ArbitratedEvent, TouchPinchArbitrator};
    use crate::{Modifiers, Point, TouchPhase, px};

    let mut arbitrator = TouchPinchArbitrator::new();
    let mod_none = Modifiers::default();

    let touch1_down = arbitrator.touch_began(1, Point::new(px(100.0), px(100.0)), mod_none);
    assert!(
        touch1_down.iter().any(|e| matches!(
            e,
            ArbitratedEvent::PlatformInput(crate::PlatformInput::PointerDown(_))
        )),
        "Touch 1 must emit pointer down"
    );

    let _ = arbitrator.touch_began(2, Point::new(px(200.0), px(100.0)), mod_none);
    let pinch_start = arbitrator.pinch_began(Point::new(px(150.0), px(100.0)), 1.0, mod_none);
    assert!(pinch_start.is_some());
    assert_eq!(pinch_start.unwrap().phase, TouchPhase::Started);

    let claim_events = arbitrator.resolve_pinch_claim(false, mod_none);
    assert!(
        claim_events.is_empty(),
        "Unclaimed pinch must not emit cancellation events"
    );
    assert!(
        !arbitrator.is_pinch_claimed(),
        "Arbitrator must not be in claimed state"
    );

    let pinch_move = arbitrator.pinch_changed(Point::new(px(150.0), px(100.0)), 1.2, mod_none);
    assert!(
        pinch_move.is_none(),
        "Unclaimed pinch should not emit pinch moved"
    );

    let touch1_move = arbitrator.touch_moved(
        1,
        Point::new(px(110.0), px(100.0)),
        Point::new(px(10.0), px(0.0)),
        mod_none,
    );
    assert!(
        touch1_move.iter().any(|e| matches!(
            e,
            ArbitratedEvent::PlatformInput(crate::PlatformInput::PointerMove(_))
        )),
        "Unclaimed touch stream must continue to emit pointer move"
    );
    assert!(
        touch1_move.iter().any(|e| matches!(
            e,
            ArbitratedEvent::PlatformInput(crate::PlatformInput::ScrollWheel(_))
        )),
        "Unclaimed touch stream must continue to emit scroll"
    );

    let touch2_end = arbitrator.touch_ended(2, Point::new(px(200.0), px(100.0)), mod_none);
    assert!(
        touch2_end.is_empty(),
        "Secondary touch end ignored in baseline"
    );

    let touch1_end = arbitrator.touch_ended(1, Point::new(px(110.0), px(100.0)), mod_none);
    assert!(
        touch1_end.iter().any(|e| matches!(
            e,
            ArbitratedEvent::PlatformInput(crate::PlatformInput::PointerUp(_))
        )),
        "Unclaimed touch 1 lift must emit normal pointer up"
    );
}

#[test]
fn pinch_arbitration_malformed_scale_and_delta_resilience() {
    use crate::touch_arbitration::TouchPinchArbitrator;
    use crate::{Modifiers, Point, TouchPhase, px};

    let mut arbitrator = TouchPinchArbitrator::new();
    let mod_none = Modifiers::default();

    let _ = arbitrator.touch_began(1, Point::new(px(100.0), px(100.0)), mod_none);
    let _ = arbitrator.touch_began(2, Point::new(px(200.0), px(100.0)), mod_none);

    let pinch_start = arbitrator.pinch_began(Point::new(px(150.0), px(100.0)), f32::NAN, mod_none);
    assert!(pinch_start.is_some());
    assert_eq!(pinch_start.unwrap().phase, TouchPhase::Started);

    let _ = arbitrator.resolve_pinch_claim(true, mod_none);

    let pinch_nan = arbitrator.pinch_changed(Point::new(px(150.0), px(100.0)), f32::NAN, mod_none);
    assert!(pinch_nan.is_some());
    let ev = pinch_nan.unwrap();
    assert!(
        ev.delta.is_finite(),
        "Delta must be finite even with NaN scale input"
    );
    assert_eq!(ev.delta, 0.0);

    let pinch_neg = arbitrator.pinch_changed(Point::new(px(150.0), px(100.0)), -5.0, mod_none);
    assert!(pinch_neg.is_some());
    let ev = pinch_neg.unwrap();
    assert!(ev.delta.is_finite());
    assert_eq!(ev.delta, 0.0);

    let pinch_inf =
        arbitrator.pinch_changed(Point::new(px(150.0), px(100.0)), f32::INFINITY, mod_none);
    assert!(pinch_inf.is_some());
    let ev = pinch_inf.unwrap();
    assert!(ev.delta.is_finite());
    assert_eq!(ev.delta, 0.0);

    let pinch_valid = arbitrator.pinch_changed(Point::new(px(150.0), px(100.0)), 1.25, mod_none);
    assert!(pinch_valid.is_some());
    let ev = pinch_valid.unwrap();
    assert!(ev.delta.is_finite());
    assert!((ev.delta - 0.25).abs() < 1e-4);
}

#[test]
fn pinch_arbitration_duplicate_raw_cancellation_idempotent() {
    use crate::touch_arbitration::{ArbitratedEvent, TouchPinchArbitrator};
    use crate::{Modifiers, Point, px};

    let mut arbitrator = TouchPinchArbitrator::new();
    let mod_none = Modifiers::default();

    let _ = arbitrator.touch_began(1, Point::new(px(100.0), px(100.0)), mod_none);
    let _ = arbitrator.touch_began(2, Point::new(px(200.0), px(100.0)), mod_none);

    let _ = arbitrator.pinch_began(Point::new(px(150.0), px(100.0)), 1.0, mod_none);
    let claim_events = arbitrator.resolve_pinch_claim(true, mod_none);

    let pointer_cancels = claim_events
        .iter()
        .filter(|e| {
            matches!(
                e,
                ArbitratedEvent::PlatformInput(crate::PlatformInput::PointerCancel(_))
            )
        })
        .count();
    assert_eq!(
        pointer_cancels, 1,
        "Pointer cancel must be emitted exactly once on claim"
    );

    let duplicate_cancel1 =
        arbitrator.touch_cancelled(1, Point::new(px(100.0), px(100.0)), mod_none);
    assert!(
        duplicate_cancel1.is_empty(),
        "Subsequent raw touch cancellation must not emit a second pointer cancel"
    );

    let duplicate_cancel2 =
        arbitrator.touch_cancelled(2, Point::new(px(200.0), px(100.0)), mod_none);
    assert!(
        duplicate_cancel2.is_empty(),
        "Secondary raw touch cancellation must not emit pointer cancel"
    );
}

#[test]
fn pinch_arbitration_scenario_trace() {
    use crate::touch_arbitration::{ArbitratedEvent, TouchPinchArbitrator};
    use crate::{Modifiers, Point, TouchPhase, px};

    let mut arbitrator = TouchPinchArbitrator::new();
    let mod_none = Modifiers::default();

    let t1_down = arbitrator.touch_began(10, Point::new(px(100.0), px(200.0)), mod_none);
    assert!(t1_down.iter().any(|e| matches!(
        e,
        ArbitratedEvent::PlatformInput(crate::PlatformInput::PointerDown(_))
    )));

    let t1_scroll = arbitrator.touch_moved(
        10,
        Point::new(px(100.0), px(190.0)),
        Point::new(px(0.0), px(-10.0)),
        mod_none,
    );
    assert!(t1_scroll.iter().any(|e| matches!(
        e,
        ArbitratedEvent::PlatformInput(crate::PlatformInput::ScrollWheel(_))
    )));

    let t2_down = arbitrator.touch_began(20, Point::new(px(100.0), px(300.0)), mod_none);
    assert!(!t2_down.iter().any(|e| matches!(
        e,
        ArbitratedEvent::PlatformInput(crate::PlatformInput::PointerDown(_))
    )));

    let pinch_start = arbitrator.pinch_began(Point::new(px(100.0), px(250.0)), 1.0, mod_none);
    assert_eq!(pinch_start.unwrap().phase, TouchPhase::Started);

    let claim_out = arbitrator.resolve_pinch_claim(true, mod_none);
    assert!(
        claim_out
            .iter()
            .any(|e| matches!(e, ArbitratedEvent::CancelFling))
    );
    assert!(claim_out.iter().any(|e| matches!(
        e,
        ArbitratedEvent::PlatformInput(crate::PlatformInput::PointerCancel(_))
    )));
    assert!(claim_out.iter().any(|e| matches!(
        e,
        ArbitratedEvent::PlatformInput(crate::PlatformInput::ScrollWheel(_))
    )));

    let pinch_move1 = arbitrator.pinch_changed(Point::new(px(100.0), px(250.0)), 1.15, mod_none);
    assert_eq!(pinch_move1.unwrap().phase, TouchPhase::Moved);

    let pinch_end = arbitrator.pinch_ended(mod_none);
    assert_eq!(pinch_end.unwrap().phase, TouchPhase::Ended);

    let t2_up = arbitrator.touch_ended(20, Point::new(px(100.0), px(350.0)), mod_none);
    assert!(t2_up.is_empty());

    let t1_drag = arbitrator.touch_moved(
        10,
        Point::new(px(100.0), px(150.0)),
        Point::new(px(0.0), px(-40.0)),
        mod_none,
    );
    assert!(t1_drag.is_empty());

    let t1_up = arbitrator.touch_ended(10, Point::new(px(100.0), px(150.0)), mod_none);
    assert!(t1_up.is_empty());

    let fresh_down = arbitrator.touch_began(30, Point::new(px(50.0), px(50.0)), mod_none);
    assert!(fresh_down.iter().any(|e| matches!(
        e,
        ArbitratedEvent::PlatformInput(crate::PlatformInput::PointerDown(_))
    )));
}
