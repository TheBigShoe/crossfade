//! Turning a Zed-format [`Keymap`] into GPUI key bindings.
//!
//! GPUI builds any registered action from its name (`cx.build_action("crossfade::Convert",
//! args)`): every `actions!` type registers itself under `namespace::Name`, so a keymap
//! file can name any of the app's actions and gpui-kit's own. A `null` entry becomes
//! GPUI's `NoAction`, which hides the bindings of the same keystrokes that it outranks,
//! the same rule Zed applies.
//!
//! GPUI cannot remove some bindings and keep others, so every apply clears the whole
//! keymap and binds again: gpui-kit's own bindings (snapshotted once, right after
//! `gpui_kit::init`), then the keymap's. Nothing is bound twice however often a file is
//! saved.

use std::rc::Rc;

use appconfig::{Keymap, Platform};

use crate::prelude::*;

/// gpui-kit's bindings as they stood after its `init`, before any of the app's.
#[derive(Clone, Default)]
pub struct KitBindings(pub Vec<KeyBinding>);

impl Global for KitBindings {}

/// Remember the bindings registered so far as gpui-kit's. Call once, right after
/// `gpui_kit::init` and before the app binds anything.
pub fn snapshot_kit_bindings(cx: &mut App) {
    let bindings: Vec<KeyBinding> = cx.key_bindings().borrow().bindings().cloned().collect();
    cx.set_global(KitBindings(bindings));
}

/// The bundled keymap for `platform`: the same actions on that platform's modifier.
pub fn bundled_keymap_text(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => include_str!("../assets/keymaps/macos.json"),
        Platform::Linux => include_str!("../assets/keymaps/linux.json"),
        Platform::Windows => include_str!("../assets/keymaps/windows.json"),
    }
}

/// GPUI bindings for `keymap`, in its order, and a message for every entry that could not
/// be used (an action no one registered, arguments the action rejects, a keystroke or
/// context that does not parse). The rest still bind.
pub fn gpui_bindings(keymap: &Keymap, cx: &App) -> (Vec<KeyBinding>, Vec<String>) {
    let mut bindings = Vec::with_capacity(keymap.entries.len());
    let mut problems = Vec::new();
    for entry in &keymap.entries {
        let place = match &entry.context {
            Some(context) => format!("\"{}\" in {context}", entry.keystrokes),
            None => format!("\"{}\"", entry.keystrokes),
        };
        let (action, input): (Box<dyn Action>, Option<SharedString>) = match &entry.action {
            None => (Box::new(NoAction), None),
            Some(action) => match cx.build_action(&action.name, action.args.clone()) {
                Ok(built) => (
                    built,
                    action.args.as_ref().map(|args| args.to_string().into()),
                ),
                Err(error) => {
                    problems.push(format!("Keymap: {place}: {error}"));
                    continue;
                }
            },
        };
        let predicate = match &entry.context {
            None => None,
            Some(context) => match KeyBindingContextPredicate::parse(context) {
                Ok(predicate) => Some(Rc::new(predicate)),
                Err(error) => {
                    problems.push(format!("Keymap: {place}: bad context: {error}"));
                    continue;
                }
            },
        };
        match KeyBinding::load(
            &entry.keystrokes,
            action,
            predicate,
            entry.use_key_equivalents,
            input,
            cx,
        ) {
            Ok(binding) => bindings.push(binding),
            Err(error) => problems.push(format!("Keymap: {place}: {error}")),
        }
    }
    (bindings, problems)
}

/// Replace every key binding with gpui-kit's, then `keymap`'s. Returns a message for each
/// entry that could not be bound.
pub fn apply(keymap: &Keymap, cx: &mut App) -> Vec<String> {
    let (bindings, problems) = gpui_bindings(keymap, cx);
    let kit = cx
        .try_global::<KitBindings>()
        .map(|kit| kit.0.clone())
        .unwrap_or_default();
    cx.clear_key_bindings();
    cx.bind_keys(kit);
    cx.bind_keys(bindings);
    problems
}
