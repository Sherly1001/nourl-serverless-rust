/// Classes for a text input, red on error. FlyonUI's `is-invalid`, not a
/// `border-error` utility: FlyonUI's hover rule outranks a bare utility, so the
/// red would vanish the moment the pointer touched it.
pub fn input_class(invalid: bool) -> &'static str {
    if invalid {
        "input h-13 text-lg is-invalid"
    } else {
        "input h-13 text-lg"
    }
}

/// The same at row scale, `is-invalid` for the reason above.
pub fn row_input_class(invalid: bool) -> &'static str {
    if invalid {
        "input input-sm is-invalid"
    } else {
        "input input-sm"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_input_marks_invalid_the_same_way() {
        assert!(row_input_class(true).contains("is-invalid"));
        assert!(!row_input_class(false).contains("is-invalid"));
    }

    #[test]
    fn the_invalid_state_uses_flyonui_is_invalid() {
        // Not `border-error`: FlyonUI's hover rule outranks a bare utility.
        assert!(input_class(true).contains("is-invalid"));
        assert!(!input_class(true).contains("border-error"));
        assert!(!input_class(false).contains("is-invalid"));
    }
}
