//! The configuration layer the desktop apps share, with no GPUI and no app-specific
//! knowledge: where the config directory lives, layered JSONC settings, a
//! comment-preserving settings and keymap editor, Zed-format keymaps and themes, and a
//! watcher that reports which kind of file changed.
//!
//! Each app supplies what is its own (names, defaults, templates, bundled themes and
//! keymaps) as arguments. This code was extracted from Serialist at commit `aff6283`.
#![forbid(unsafe_code)]

pub mod atomic;

#[cfg(test)]
mod test_util;
