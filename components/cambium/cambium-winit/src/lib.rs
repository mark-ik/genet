/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Platform integration for Cambium applications hosted with winit.
//!
//! Window presentation remains the host's responsibility. This crate translates
//! winit's keyboard vocabulary into Cambium events and maps its wheel deltas to
//! scroll axes.
//!
//! Deliberately thin, and thin on purpose: it depends on `cambium` and `winit`
//! and nothing else, so it can be published and reached from a consumer that
//! resolves the catalog from the registry. The accessibility host moved to
//! `cambium-winit-a11y` on 2026-07-26 for exactly that reason -- it needs the
//! laid-out genet DOM and the platform adapter, neither of which can be
//! published, and holding them here made this crate unpublishable too. A host
//! that wants a screen-reader tree takes both crates.

pub mod scroll;

pub use scroll::{ScrollbarFade, wheel_axes};

use cambium::{CompositionEvent, Key, KeyEvent, Modifiers, NamedKey};
use winit::event::Ime;
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey as WinitNamedKey};

/// Map a winit logical key and modifiers into a Cambium event.
///
/// Dead and unidentified keys have no routable Cambium representation.
pub fn key_event_from_winit(key: &WinitKey, mods: Modifiers) -> Option<KeyEvent> {
    let mapped = match key {
        WinitKey::Character(s) => Key::Character(s.to_string()),
        WinitKey::Named(named) => Key::Named(match named {
            WinitNamedKey::Backspace => NamedKey::Backspace,
            WinitNamedKey::Enter => NamedKey::Enter,
            WinitNamedKey::Tab => NamedKey::Tab,
            WinitNamedKey::Escape => NamedKey::Escape,
            WinitNamedKey::Space => NamedKey::Space,
            WinitNamedKey::ArrowLeft => NamedKey::ArrowLeft,
            WinitNamedKey::ArrowRight => NamedKey::ArrowRight,
            WinitNamedKey::ArrowUp => NamedKey::ArrowUp,
            WinitNamedKey::ArrowDown => NamedKey::ArrowDown,
            WinitNamedKey::Delete => NamedKey::Delete,
            WinitNamedKey::Home => NamedKey::Home,
            WinitNamedKey::End => NamedKey::End,
            WinitNamedKey::PageUp => NamedKey::PageUp,
            WinitNamedKey::PageDown => NamedKey::PageDown,
            _ => NamedKey::Other,
        }),
        WinitKey::Dead(_) | WinitKey::Unidentified(_) => return None,
    };
    Some(KeyEvent::with_mods(mapped, mods))
}

/// Map winit's modifier state into Cambium's platform-neutral modifiers.
pub fn modifiers_from_winit(state: ModifiersState) -> Modifiers {
    Modifiers {
        shift: state.shift_key(),
        ctrl: state.control_key(),
        alt: state.alt_key(),
        meta: state.super_key(),
    }
}

/// Map a winit IME lifecycle event into the same focused Cambium event channel
/// as keyboard input. A host sends the result to `runner.dispatch_key`; the
/// focused text field consumes it while other controls ignore it.
pub fn ime_event_from_winit(ime: &Ime) -> KeyEvent {
    let composition = match ime {
        Ime::Enabled => CompositionEvent::Enabled,
        Ime::Preedit(text, selection) => CompositionEvent::Preedit {
            text: text.clone(),
            selection: *selection,
        },
        Ime::Commit(text) => CompositionEvent::Commit(text.clone()),
        Ime::Disabled => CompositionEvent::Disabled,
    };
    KeyEvent::new(Key::Composition(composition))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_key_preserves_modifiers() {
        let mods = Modifiers {
            shift: true,
            ctrl: false,
            alt: false,
            meta: false,
        };
        let event = key_event_from_winit(&WinitKey::Character("A".into()), mods)
            .expect("a character key should map");

        assert!(matches!(event.key, Key::Character(ref value) if value == "A"));
        assert_eq!(event.mods, mods);
    }

    #[test]
    fn named_and_unidentified_keys_use_explicit_fallbacks() {
        let enter =
            key_event_from_winit(&WinitKey::Named(WinitNamedKey::Enter), Modifiers::default())
                .expect("Enter should map");
        assert!(matches!(enter.key, Key::Named(NamedKey::Enter)));

        assert!(
            key_event_from_winit(
                &WinitKey::Unidentified(winit::keyboard::NativeKey::Unidentified),
                Modifiers::default(),
            )
            .is_none()
        );
    }

    #[test]
    fn ime_preedit_and_commit_preserve_platform_payloads() {
        let preedit = ime_event_from_winit(&Ime::Preedit("かな".to_owned(), Some((3, 6))));
        assert!(matches!(
            preedit.key,
            Key::Composition(CompositionEvent::Preedit {
                ref text,
                selection: Some((3, 6)),
            }) if text == "かな"
        ));

        let commit = ime_event_from_winit(&Ime::Commit("仮名".to_owned()));
        assert!(matches!(
            commit.key,
            Key::Composition(CompositionEvent::Commit(ref text)) if text == "仮名"
        ));
    }
}
