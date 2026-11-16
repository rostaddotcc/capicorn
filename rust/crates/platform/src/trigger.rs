//! Trigger syntax translation: human/X11-ish combos → XDG shortcuts spec.
//!
//! Ported verbatim from the live `src-tauri/src/hotkeys.rs` (`xdg_trigger`,
//! `normalize_key`), which is what the compositor accepted in practice.

/// Translate `"CTRL+SHIFT+P"` / `"Print"` / `"SHIFT+Print"` into the XDG
/// "shortcuts" spec form (`"<Control><Shift>p"`, `"Print"`, `"<Shift>Print"`).
///
/// Input that already looks like an XDG accelerator (a `<` somewhere) is
/// returned unchanged.
pub fn xdg_trigger(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // `<Control><Shift>p` — already in spec form.
    if trimmed.contains('<') {
        return trimmed.to_string();
    }

    let mut out = String::new();
    let mut key: Option<String> = None;

    for raw in trimmed.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "ctl" => out.push_str("<Control>"),
            "shift" => out.push_str("<Shift>"),
            "alt" | "option" | "opt" => out.push_str("<Alt>"),
            "super" | "meta" | "win" | "cmd" | "command" => out.push_str("<Super>"),
            _ => key = Some(normalize_key(token)),
        }
    }

    match key {
        // Modifiers only (a bad config): fall back to the raw text rather than
        // binding every key on the keyboard.
        None => trimmed.to_string(),
        Some(key) => {
            out.push_str(&key);
            out
        }
    }
}

/// Keysym-ish normalisation: single characters are lowercased (the spec's
/// examples use `p`, not `P`), named keys keep their canonical spelling,
/// everything else is kept verbatim.
fn normalize_key(token: &str) -> String {
    if token.chars().count() == 1 {
        // Letters only; digits and punctuation have no case.
        return token.to_ascii_lowercase();
    }
    match token.to_ascii_lowercase().as_str() {
        "print" | "printscreen" | "prtsc" => "Print".to_string(),
        "space" => "space".to_string(),
        "escape" | "esc" => "Escape".to_string(),
        "return" | "enter" => "Return".to_string(),
        "tab" => "Tab".to_string(),
        "delete" | "del" => "Delete".to_string(),
        "backspace" => "BackSpace".to_string(),
        "insert" | "ins" => "Insert".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" | "pgup" => "Page_Up".to_string(),
        "pagedown" | "pgdn" => "Page_Down".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        other => {
            // `f1`..`f12`, `Print`-style names we do not know: uppercase the
            // leading letter so `f5` becomes `F5` and `Print` stays `Print`.
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_converts_modifiers_to_xdg_axes() {
        assert_eq!(xdg_trigger("CTRL+SHIFT+P"), "<Control><Shift>p");
        assert_eq!(xdg_trigger("ctrl+p"), "<Control>p");
        assert_eq!(xdg_trigger("ALT+F5"), "<Alt>F5");
        assert_eq!(xdg_trigger("Super+Print"), "<Super>Print");
    }

    #[test]
    fn trigger_handles_bare_named_keys() {
        assert_eq!(xdg_trigger("Print"), "Print");
        assert_eq!(xdg_trigger("SHIFT+Print"), "<Shift>Print");
        assert_eq!(xdg_trigger("  ESCAPE  "), "Escape");
        assert_eq!(xdg_trigger("Ctrl+Space"), "<Control>space");
        assert_eq!(xdg_trigger("Ctrl+PageDown"), "<Control>Page_Down");
    }

    #[test]
    fn trigger_passes_through_existing_xdg_strings() {
        assert_eq!(xdg_trigger("<Control><Shift>p"), "<Control><Shift>p");
        assert_eq!(xdg_trigger("<Super>Print"), "<Super>Print");
    }

    #[test]
    fn trigger_never_emits_a_modifier_only_binding() {
        // A config that is only modifiers must not bind "any key".
        assert_eq!(xdg_trigger("CTRL+SHIFT"), "CTRL+SHIFT");
        assert_eq!(xdg_trigger(""), "");
    }

    #[test]
    fn the_shipped_defaults_are_valid_spec_triggers() {
        assert_eq!(xdg_trigger("Print"), "Print");
        assert_eq!(xdg_trigger("Shift+Print"), "<Shift>Print");
        // Idempotent: a spec string survives a second pass untouched.
        for t in ["Print", "Shift+Print", "Ctrl+Shift+X"] {
            let once = xdg_trigger(t);
            assert_eq!(xdg_trigger(&once), once);
        }
    }
}
