//! Key names for bindings: winit's logical key names (`PageDown`, `ArrowRight`, `F1`,
//! `Space`, `Clear`) or the character itself (`.`, `b`).

use winit::keyboard::Key;

pub fn key_name(key: &Key) -> Option<String> {
    match key {
        Key::Named(n) => Some(format!("{n:?}")),
        Key::Character(s) => Some(s.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::NamedKey;

    #[test]
    fn names_match_default_bindings() {
        let k = pp6core::KeyBindings::default();
        for (key, cmd) in [
            (Key::Named(NamedKey::PageDown), pp6core::Command::Next),
            (Key::Named(NamedKey::ArrowLeft), pp6core::Command::Prev),
            (Key::Named(NamedKey::Space), pp6core::Command::Next),
            (Key::Named(NamedKey::F1), pp6core::Command::ClearAll),
            (Key::Named(NamedKey::F2), pp6core::Command::ClearBackground),
            (Key::Named(NamedKey::F3), pp6core::Command::ClearText),
            (Key::Character(".".into()), pp6core::Command::ClearAll),
        ] {
            assert_eq!(
                key_name(&key).and_then(|n| k.command_for(&n)),
                Some(cmd),
                "{key:?}"
            );
        }
    }
}
