/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Platform input translation for Cambium applications hosted with winit.
//!
//! Window presentation remains the host's responsibility. This crate only
//! translates winit's keyboard vocabulary into Cambium events.

use cambium::{Key, KeyEvent, Modifiers, NamedKey};
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
}
