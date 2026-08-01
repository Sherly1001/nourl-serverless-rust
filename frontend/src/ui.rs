/// Classes for a text input, red when it is showing an error.
///
/// The error state is FlyonUI's own `is-invalid` rather than a `border-error`
/// utility. FlyonUI styles hover with
/// `.input:hover:not(:focus,:focus-within)`, whose specificity (0,3,0) beats
/// any single utility class — so a `border-error` input loses its red border
/// the moment the pointer touches it. `is-invalid` comes with matching base,
/// hover and focus rules, so the colour survives all three.
pub fn input_class(invalid: bool) -> &'static str {
    if invalid {
        "input h-13 text-lg is-invalid"
    } else {
        "input h-13 text-lg"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_invalid_state_uses_flyonui_is_invalid() {
        // Not `border-error`: FlyonUI's hover rule outranks a bare utility.
        assert!(input_class(true).contains("is-invalid"));
        assert!(!input_class(true).contains("border-error"));
        assert!(!input_class(false).contains("is-invalid"));
    }
}
