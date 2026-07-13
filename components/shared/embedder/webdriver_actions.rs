/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The WebDriver Actions tick interpreter: spec action sequences in,
//! per-tick [`InputEvent`]s out.
//!
//! This is the *one* interpreter the native-automation plan puts beneath both
//! consumers: the classic-protocol adapter (phase 4) hands it the Actions JSON a
//! remote client sent, and `genet-wpt`'s `test_driver_internal.action_sequence`
//! hands it the same JSON from inside a test page. It lives beside
//! `webdriver.rs` because its input types are the pinned `webdriver` crate's
//! (already a dependency here) and its output is this crate's [`InputEvent`]
//! vocabulary — the same events a host's real input path produces, delivered
//! through the surviving `WebDriverCommandMsg::InputEvent` seam or a host's own
//! dispatch.
//!
//! Interpretation is pure: no clock, no DOM. Two consequences:
//!
//! - **Element origins are resolved by the caller.** A `PointerOrigin::Element`
//!   names a node only the caller can locate (via the semantic projection or a
//!   handle registry); the interpreter takes a resolver closure and fails
//!   loudly on an element it cannot resolve, never guessing a position.
//! - **Time is reported, not slept.** Each tick carries its spec duration (the
//!   max of its actions' durations); the consumer decides whether to honor it
//!   (a headed run pacing real frames) or collapse it (a headless test).
//!
//! Deliberate first-cut deviations from the spec, each stated where it bites:
//! pointer moves emit a single event at the final position rather than
//! interpolated intermediates; key events do not maintain a modifier state
//! (shift does not uppercase subsequent characters). Pen pointers are
//! interpreted as mouse.
//!
//! **Pointer types.** `pointerType: "mouse"` (and `"pen"`) emit mouse events;
//! `"touch"` emits touch events, because a touch pointer is a finger, not a
//! cursor. Two consequences fall out of that and are load-bearing (WPT's
//! `dom/events/non-cancelable-when-passive` depends on both):
//! - A touch pointer that is **not down emits nothing on a move**. There is no
//!   "hovering finger"; the spec's own `injectInput` idiom moves a touch pointer
//!   to its origin *before* pressing, and that move must not fabricate a
//!   `touchmove`. Position is still tracked, so the subsequent press lands right.
//! - Touch events carry no button; a touch source's down/up are `Down`/`Up`
//!   touch events keyed by the source's touch id (its input-source index, which
//!   is stable for the transaction).

use euclid::Point2D;
use keyboard_types::{Key, KeyState, NamedKey};
use webdriver::actions::{
    ActionSequence, ActionsType, GeneralAction, KeyAction, KeyActionItem, PointerAction,
    PointerActionItem, PointerOrigin, PointerType, WheelAction, WheelActionItem,
};

use crate::input_events::{
    InputEvent, KeyboardEvent, MouseButtonAction, MouseButtonEvent, MouseMoveEvent, TouchEvent,
    TouchEventType, TouchId, WheelDelta, WheelEvent, WheelMode,
};
use crate::WebViewPoint;

/// One tick of an actions transaction: the events every input source emitted at
/// this index, in source order, plus the tick's spec duration.
#[derive(Debug)]
pub struct ActionsTick {
    /// The tick's duration in ms — the max of its actions' durations. Reported
    /// for the consumer to honor or collapse, never slept here.
    pub duration_ms: u64,
    pub events: Vec<InputEvent>,
}

/// Why a sequence could not be interpreted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionsError {
    /// A pointer or wheel origin named an element the caller's resolver could
    /// not locate. Carries the element reference verbatim.
    UnresolvedElementOrigin(String),
}

/// Interpret spec action sequences into per-tick input events.
///
/// `sequences` is the transaction's parallel input sources; per the spec, tick
/// `i` performs item `i` of every source at once (a source with fewer items
/// idles). `resolve_element` maps an element reference (the spec's opaque
/// element id) to its in-view center in page (CSS-pixel) coordinates — the
/// caller owns that lookup because only the caller has the tree.
pub fn interpret_actions(
    sequences: &[ActionSequence],
    resolve_element: &dyn Fn(&str) -> Option<(f64, f64)>,
) -> Result<Vec<ActionsTick>, ActionsError> {
    let tick_count = sequences
        .iter()
        .map(|sequence| match &sequence.actions {
            ActionsType::Null { actions } => actions.len(),
            ActionsType::Key { actions } => actions.len(),
            ActionsType::Pointer { actions, .. } => actions.len(),
            ActionsType::Wheel { actions } => actions.len(),
        })
        .max()
        .unwrap_or(0);

    // Per-pointer-source position state, keyed by sequence index: `pointer`
    // origins are relative to where that source last moved.
    let mut positions: Vec<(f64, f64)> = vec![(0.0, 0.0); sequences.len()];
    // Per-pointer-source pressed state, keyed the same way. Only touch reads it
    // (a finger that is not down emits nothing on a move, and cannot lift); a
    // mouse hovers and moves freely, so it ignores this.
    let mut down_sources: Vec<bool> = vec![false; sequences.len()];

    let mut ticks = Vec::with_capacity(tick_count);
    for tick_index in 0..tick_count {
        let mut duration_ms = 0u64;
        let mut events = Vec::new();

        for (source_index, sequence) in sequences.iter().enumerate() {
            match &sequence.actions {
                ActionsType::Null { actions } => {
                    if let Some(NullOrPause::Pause(pause)) =
                        actions.get(tick_index).map(|item| match item {
                            webdriver::actions::NullActionItem::General(GeneralAction::Pause(
                                pause,
                            )) => NullOrPause::Pause(pause.duration.unwrap_or(0)),
                        })
                    {
                        duration_ms = duration_ms.max(pause);
                    }
                },
                ActionsType::Key { actions } => match actions.get(tick_index) {
                    Some(KeyActionItem::General(GeneralAction::Pause(pause))) => {
                        duration_ms = duration_ms.max(pause.duration.unwrap_or(0));
                    },
                    Some(KeyActionItem::Key(action)) => {
                        let (state, value) = match action {
                            KeyAction::Down(down) => (KeyState::Down, &down.value),
                            KeyAction::Up(up) => (KeyState::Up, &up.value),
                        };
                        events.push(InputEvent::Keyboard(KeyboardEvent::from_state_and_key(
                            state,
                            key_from_webdriver(value),
                        )));
                    },
                    None => {},
                },
                ActionsType::Pointer {
                    actions,
                    parameters,
                } => {
                    let is_touch = parameters.pointer_type == PointerType::Touch;
                    let touch_id = TouchId(source_index as i32);
                    match actions.get(tick_index) {
                        Some(PointerActionItem::General(GeneralAction::Pause(pause))) => {
                            duration_ms = duration_ms.max(pause.duration.unwrap_or(0));
                        },
                        Some(PointerActionItem::Pointer(action)) => match action {
                            PointerAction::Down(down) => {
                                down_sources[source_index] = true;
                                events.push(if is_touch {
                                    InputEvent::Touch(TouchEvent::new(
                                        TouchEventType::Down,
                                        touch_id,
                                        page_point(positions[source_index]),
                                    ))
                                } else {
                                    InputEvent::MouseButton(MouseButtonEvent::new(
                                        MouseButtonAction::Down,
                                        down.button.into(),
                                        page_point(positions[source_index]),
                                    ))
                                });
                            },
                            PointerAction::Up(up) => {
                                let was_down = down_sources[source_index];
                                down_sources[source_index] = false;
                                events.push(if is_touch {
                                    // A finger that never touched down cannot lift.
                                    if !was_down {
                                        continue;
                                    }
                                    InputEvent::Touch(TouchEvent::new(
                                        TouchEventType::Up,
                                        touch_id,
                                        page_point(positions[source_index]),
                                    ))
                                } else {
                                    InputEvent::MouseButton(MouseButtonEvent::new(
                                        MouseButtonAction::Up,
                                        up.button.into(),
                                        page_point(positions[source_index]),
                                    ))
                                });
                            },
                            PointerAction::Move(move_action) => {
                                duration_ms = duration_ms.max(move_action.duration.unwrap_or(0));
                                let target = resolve_origin(
                                    &move_action.origin,
                                    (move_action.x, move_action.y),
                                    positions[source_index],
                                    resolve_element,
                                )?;
                                positions[source_index] = target;
                                // Spec allows interpolated intermediate events over
                                // the duration; this emits the final position once.
                                if is_touch {
                                    // No hovering finger: a touch pointer that is not
                                    // down emits nothing. The position is still
                                    // tracked above, so the next press lands here.
                                    if down_sources[source_index] {
                                        events.push(InputEvent::Touch(TouchEvent::new(
                                            TouchEventType::Move,
                                            touch_id,
                                            page_point(target),
                                        )));
                                    }
                                } else {
                                    events.push(InputEvent::MouseMove(MouseMoveEvent::new(
                                        page_point(target),
                                    )));
                                }
                            },
                            // Cancel rewinds a source's effects mid-transaction; no
                            // consumer needs it yet, and a silent partial rewind
                            // would be worse than none.
                            PointerAction::Cancel => {},
                        },
                        None => {},
                    }
                },
                ActionsType::Wheel { actions } => match actions.get(tick_index) {
                    Some(WheelActionItem::General(GeneralAction::Pause(pause))) => {
                        duration_ms = duration_ms.max(pause.duration.unwrap_or(0));
                    },
                    Some(WheelActionItem::Wheel(WheelAction::Scroll(scroll))) => {
                        duration_ms = duration_ms.max(scroll.duration.unwrap_or(0));
                        let at = resolve_origin(
                            &scroll.origin,
                            (
                                scroll.x.unwrap_or(0) as f64,
                                scroll.y.unwrap_or(0) as f64,
                            ),
                            positions[source_index],
                            resolve_element,
                        )?;
                        // Sign flip: the spec's positive deltaY scrolls the page
                        // down; this crate's positive WheelDelta scrolls the view
                        // up (revealing content above).
                        events.push(InputEvent::Wheel(WheelEvent::new(
                            WheelDelta {
                                x: -(scroll.deltaX.unwrap_or(0) as f64),
                                y: -(scroll.deltaY.unwrap_or(0) as f64),
                                z: 0.0,
                                mode: WheelMode::DeltaPixel,
                            },
                            page_point(at),
                        )));
                    },
                    None => {},
                },
            }
        }

        ticks.push(ActionsTick {
            duration_ms,
            events,
        });
    }
    Ok(ticks)
}

enum NullOrPause {
    Pause(u64),
}

fn page_point((x, y): (f64, f64)) -> WebViewPoint {
    WebViewPoint::Page(Point2D::new(x as f32, y as f32))
}

fn resolve_origin(
    origin: &PointerOrigin,
    (x, y): (f64, f64),
    current: (f64, f64),
    resolve_element: &dyn Fn(&str) -> Option<(f64, f64)>,
) -> Result<(f64, f64), ActionsError> {
    match origin {
        PointerOrigin::Viewport => Ok((x, y)),
        PointerOrigin::Pointer => Ok((current.0 + x, current.1 + y)),
        PointerOrigin::Element(element) => {
            let center = resolve_element(&element.0)
                .ok_or_else(|| ActionsError::UnresolvedElementOrigin(element.0.clone()))?;
            Ok((center.0 + x, center.1 + y))
        },
    }
}

/// A WebDriver key action's value (one grapheme) to a `keyboard_types::Key`.
/// The spec assigns Unicode private-use codepoints (`\u{E000}`…) to named keys;
/// the common subset is mapped here, an unknown private-use codepoint becomes
/// `Unidentified` (loud in a test, harmless in a page), and anything else is
/// the printable character itself.
fn key_from_webdriver(value: &str) -> Key {
    let mut chars = value.chars();
    let (Some(first), None) = (chars.next(), chars.next()) else {
        return Key::Character(value.to_string());
    };
    let named = match first {
        '\u{E003}' => NamedKey::Backspace,
        '\u{E004}' => NamedKey::Tab,
        '\u{E006}' | '\u{E007}' => NamedKey::Enter,
        '\u{E008}' => NamedKey::Shift,
        '\u{E009}' => NamedKey::Control,
        '\u{E00A}' => NamedKey::Alt,
        '\u{E00C}' => NamedKey::Escape,
        '\u{E00D}' => return Key::Character(" ".to_string()),
        '\u{E010}' => NamedKey::End,
        '\u{E011}' => NamedKey::Home,
        '\u{E012}' => NamedKey::ArrowLeft,
        '\u{E013}' => NamedKey::ArrowUp,
        '\u{E014}' => NamedKey::ArrowRight,
        '\u{E015}' => NamedKey::ArrowDown,
        '\u{E017}' => NamedKey::Delete,
        '\u{E03D}' => NamedKey::Meta,
        '\u{E000}'..='\u{F8FF}' => NamedKey::Unidentified,
        _ => return Key::Character(first.to_string()),
    };
    Key::Named(named)
}

#[cfg(test)]
mod tests {
    use webdriver::actions::{
        KeyDownAction, KeyUpAction, PauseAction, PointerActionParameters, PointerDownAction,
        PointerMoveAction, PointerUpAction,
    };
    use webdriver::common::WebElement;

    use super::*;
    use crate::input_events::MouseButton;

    fn no_elements(_: &str) -> Option<(f64, f64)> {
        None
    }

    /// The canonical "click this element" transaction thirtyfour and
    /// test_driver both emit: move-to-element, down, up — three ticks, the
    /// move landing at the element's resolved center and the press happening
    /// where the pointer now is.
    #[test]
    fn element_click_resolves_move_down_up() {
        let sequence = ActionSequence {
            id: "mouse".into(),
            actions: ActionsType::Pointer {
                parameters: PointerActionParameters::default(),
                actions: vec![
                    PointerActionItem::Pointer(PointerAction::Move(PointerMoveAction {
                        origin: PointerOrigin::Element(WebElement("el-7".into())),
                        x: 0.0,
                        y: 0.0,
                        ..PointerMoveAction::default()
                    })),
                    PointerActionItem::Pointer(PointerAction::Down(PointerDownAction {
                        button: 0,
                        ..PointerDownAction::default()
                    })),
                    PointerActionItem::Pointer(PointerAction::Up(PointerUpAction {
                        button: 0,
                        ..PointerUpAction::default()
                    })),
                ],
            },
        };
        let resolve = |id: &str| (id == "el-7").then_some((120.0, 40.0));
        let ticks = interpret_actions(std::slice::from_ref(&sequence), &resolve).unwrap();

        assert_eq!(ticks.len(), 3);
        let expected = WebViewPoint::Page(Point2D::new(120.0, 40.0));
        assert!(matches!(
            ticks[0].events[..],
            [InputEvent::MouseMove(MouseMoveEvent { point, .. })] if point == expected
        ));
        assert!(matches!(
            ticks[1].events[..],
            [InputEvent::MouseButton(MouseButtonEvent {
                action: MouseButtonAction::Down,
                button: MouseButton::Left,
                point,
            })] if point == expected
        ));
        assert!(matches!(
            ticks[2].events[..],
            [InputEvent::MouseButton(MouseButtonEvent {
                action: MouseButtonAction::Up,
                ..
            })]
        ));
    }

    /// An element origin the caller cannot resolve is an error, not a guess:
    /// clicking at a made-up position is exactly the coordinate-drift failure
    /// the semantic layer exists to end.
    #[test]
    fn unresolvable_element_origin_fails_loudly() {
        let sequence = ActionSequence {
            id: "mouse".into(),
            actions: ActionsType::Pointer {
                parameters: PointerActionParameters::default(),
                actions: vec![PointerActionItem::Pointer(PointerAction::Move(
                    PointerMoveAction {
                        origin: PointerOrigin::Element(WebElement("gone".into())),
                        ..PointerMoveAction::default()
                    },
                ))],
            },
        };
        assert!(matches!(
            interpret_actions(std::slice::from_ref(&sequence), &no_elements),
            Err(ActionsError::UnresolvedElementOrigin(id)) if id == "gone"
        ));
    }

    /// Pointer-relative moves accumulate across ticks; parallel sources zip
    /// into the same tick (the spec's lockstep model), and a pause sets the
    /// tick's duration without emitting anything.
    #[test]
    fn relative_moves_accumulate_and_sources_zip() {
        let pointer = ActionSequence {
            id: "mouse".into(),
            actions: ActionsType::Pointer {
                parameters: PointerActionParameters::default(),
                actions: vec![
                    PointerActionItem::Pointer(PointerAction::Move(PointerMoveAction {
                        origin: PointerOrigin::Viewport,
                        x: 100.0,
                        y: 50.0,
                        ..PointerMoveAction::default()
                    })),
                    PointerActionItem::Pointer(PointerAction::Move(PointerMoveAction {
                        origin: PointerOrigin::Pointer,
                        x: 10.0,
                        y: -5.0,
                        ..PointerMoveAction::default()
                    })),
                ],
            },
        };
        let keys = ActionSequence {
            id: "kb".into(),
            actions: ActionsType::Key {
                actions: vec![
                    KeyActionItem::General(GeneralAction::Pause(PauseAction {
                        duration: Some(250),
                    })),
                    KeyActionItem::Key(KeyAction::Down(KeyDownAction { value: "a".into() })),
                ],
            },
        };
        let ticks = interpret_actions(&[pointer, keys], &no_elements).unwrap();

        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0].duration_ms, 250, "the pause paces its whole tick");
        assert_eq!(ticks[0].events.len(), 1, "tick 0: the absolute move only");
        // Tick 1: the relative move lands at 110, 45 and the key lands beside it.
        let expected = WebViewPoint::Page(Point2D::new(110.0, 45.0));
        assert!(matches!(
            ticks[1].events[..],
            [
                InputEvent::MouseMove(MouseMoveEvent { point, .. }),
                InputEvent::Keyboard(_),
            ] if point == expected
        ));
    }

    /// Key values map per the spec's private-use table: named keys for the
    /// mapped subset, `Unidentified` for an unmapped private-use codepoint,
    /// the character itself otherwise. Up mirrors down.
    #[test]
    fn key_values_map_to_keyboard_types() {
        let seq = ActionSequence {
            id: "kb".into(),
            actions: ActionsType::Key {
                actions: vec![
                    KeyActionItem::Key(KeyAction::Down(KeyDownAction {
                        value: "\u{E007}".into(),
                    })),
                    KeyActionItem::Key(KeyAction::Up(KeyUpAction {
                        value: "\u{E007}".into(),
                    })),
                    KeyActionItem::Key(KeyAction::Down(KeyDownAction { value: "q".into() })),
                    KeyActionItem::Key(KeyAction::Down(KeyDownAction {
                        value: "\u{E0FF}".into(),
                    })),
                ],
            },
        };
        let ticks = interpret_actions(&[seq], &no_elements).unwrap();
        let key = |tick: &ActionsTick| match &tick.events[..] {
            [InputEvent::Keyboard(event)] => (event.event.state, event.event.key.clone()),
            other => panic!("expected one keyboard event, got {other:?}"),
        };
        assert_eq!(
            key(&ticks[0]),
            (KeyState::Down, Key::Named(NamedKey::Enter))
        );
        assert_eq!(key(&ticks[1]), (KeyState::Up, Key::Named(NamedKey::Enter)));
        assert_eq!(
            key(&ticks[2]),
            (KeyState::Down, Key::Character("q".into()))
        );
        assert_eq!(
            key(&ticks[3]),
            (KeyState::Down, Key::Named(NamedKey::Unidentified))
        );
    }

    /// Wheel deltas flip sign crossing the vocabulary boundary: the spec's
    /// positive deltaY scrolls the page down; this crate's positive delta
    /// scrolls the view up.
    #[test]
    fn wheel_scroll_flips_delta_signs() {
        use webdriver::actions::WheelScrollAction;
        let seq = ActionSequence {
            id: "wheel".into(),
            actions: ActionsType::Wheel {
                actions: vec![WheelActionItem::Wheel(WheelAction::Scroll(
                    WheelScrollAction {
                        origin: PointerOrigin::Viewport,
                        x: Some(200),
                        y: Some(100),
                        deltaX: Some(0),
                        deltaY: Some(120),
                        duration: None,
                    },
                ))],
            },
        };
        let ticks = interpret_actions(&[seq], &no_elements).unwrap();
        match &ticks[0].events[..] {
            [InputEvent::Wheel(WheelEvent { delta, point })] => {
                assert_eq!(delta.y, -120.0, "spec down-scroll is a negative view delta");
                assert_eq!(delta.x, 0.0);
                assert_eq!(*point, WebViewPoint::Page(Point2D::new(200.0, 100.0)));
            },
            other => panic!("expected one wheel event, got {other:?}"),
        }
    }
}
