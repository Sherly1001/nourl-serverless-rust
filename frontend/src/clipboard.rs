use leptos::task::spawn_local;
use wasm_bindgen_futures::JsFuture;

/// This page's origin, for building an absolute short link.
pub fn origin() -> String {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default()
}

/// The shareable form of a code. Kept separate from `origin()` so it can be
/// tested without a window.
pub fn short_link(origin: &str, code: &str) -> String {
    format!("{}/{}", origin.trim_end_matches('/'), code)
}

/// Writes `text` to the clipboard. Fire and forget: the browser either allows
/// it or does not, and there is nothing useful to do about a refusal.
pub fn copy(text: String) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let clipboard = window.navigator().clipboard();
    spawn_local(async move {
        let _ = JsFuture::from(clipboard.write_text(&text)).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_link_joins_origin_and_code_once() {
        assert_eq!(
            short_link("https://nourl.space", "abc"),
            "https://nourl.space/abc"
        );
        // A trailing slash on the origin must not double up.
        assert_eq!(
            short_link("https://nourl.space/", "abc"),
            "https://nourl.space/abc"
        );
    }
}
