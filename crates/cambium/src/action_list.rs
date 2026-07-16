/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Compatibility names for the original searchable action list.
//!
//! The implementation now delegates to the shared command-surface engine. New
//! code can use [`crate::command_palette`] and the canonical `Command*` names;
//! existing consumers retain their source-compatible API.

use crate::{
    Action, CommandEvent, CommandItem, CommandState, GenetCtx, GenetElement, View, command_palette,
    map_action,
};

pub type ActionItem = CommandItem;
pub type ActionListState = CommandState;

/// Compatibility event emitted by [`action_list`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionListEvent {
    Activate(usize),
    Dismiss,
}

impl Action for ActionListEvent {}

/// Render the original palette-shaped action list through [`command_palette`].
pub fn action_list(
    state: &ActionListState,
    items: &[ActionItem],
) -> impl View<ActionListState, ActionListEvent, GenetCtx, Element = GenetElement> + use<> {
    map_action(command_palette(state, items), |_state, event| match event {
        CommandEvent::Activate(path) => {
            ActionListEvent::Activate(path.first().copied().unwrap_or(0))
        }
        CommandEvent::Dismiss => ActionListEvent::Dismiss,
    })
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use genet_scripted_dom::ScriptedDom;

    use super::*;
    use crate::{DomHandle, GenetAppRunner, Key, KeyEvent, NamedKey};

    #[test]
    fn keyboard_filters_moves_and_activates_original_index() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::new(
            dom,
            |state: &ActionListState| {
                action_list(
                    state,
                    &[
                        ActionItem::new("Open"),
                        ActionItem::new("Close").disabled(true),
                        ActionItem::new("Copy"),
                    ],
                )
            },
            ActionListState::default(),
        );
        runner.set_focus(Some(runner.root()));

        runner.dispatch_key(KeyEvent::new(Key::Character("o".into())));
        assert_eq!(runner.state().query, "o");

        let actions = runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowDown)));
        assert!(actions.is_empty());
        let actions = runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Enter)));
        assert_eq!(actions, [ActionListEvent::Activate(2)]);
    }
}
