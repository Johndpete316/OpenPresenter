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
        let k = opcore::KeyBindings::default();
        for (key, cmd) in [
            (Key::Named(NamedKey::PageDown), opcore::Command::Next),
            (Key::Named(NamedKey::ArrowLeft), opcore::Command::Prev),
            (Key::Named(NamedKey::Space), opcore::Command::Next),
            (Key::Named(NamedKey::F1), opcore::Command::ClearAll),
            (Key::Named(NamedKey::F2), opcore::Command::ClearBackground),
            (Key::Named(NamedKey::F3), opcore::Command::ClearText),
            (Key::Named(NamedKey::F4), opcore::Command::FreezeToggle),
            (Key::Named(NamedKey::F5), opcore::Command::BlankToggle),
            (Key::Character(".".into()), opcore::Command::ClearAll),
        ] {
            assert_eq!(
                key_name(&key).and_then(|n| k.command_for(&n)),
                Some(cmd),
                "{key:?}"
            );
        }
    }
}
