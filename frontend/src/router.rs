use leptos::prelude::*;
use wasm_bindgen::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Route {
    Shorten,
    Login,
    MyUrls,
    NotFound,
}

fn read_hash() -> String {
    web_sys::window()
        .and_then(|w| w.location().hash().ok())
        .unwrap_or_default()
}

pub fn parse_route(hash: &str) -> Route {
    match hash.trim_start_matches('#').trim_start_matches('/') {
        "" => Route::Shorten,
        "login" => Route::Login,
        "urls" => Route::MyUrls,
        _ => Route::NotFound,
    }
}

pub fn use_hash_route() -> Signal<Route> {
    let (hash, set_hash) = signal(read_hash());
    let closure = Closure::<dyn FnMut()>::new(move || set_hash.set(read_hash()));
    if let Some(window) = web_sys::window() {
        let _ =
            window.add_event_listener_with_callback("hashchange", closure.as_ref().unchecked_ref());
    }
    closure.forget();
    Signal::derive(move || parse_route(&hash.get()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_routes() {
        assert_eq!(parse_route(""), Route::Shorten);
        assert_eq!(parse_route("#/"), Route::Shorten);
        assert_eq!(parse_route("#"), Route::Shorten);
        assert_eq!(parse_route("#/whatever"), Route::NotFound);
    }

    #[test]
    fn parses_the_authenticated_routes() {
        assert_eq!(parse_route("#/login"), Route::Login);
        assert_eq!(parse_route("#/urls"), Route::MyUrls);
        assert_eq!(parse_route("#/nope"), Route::NotFound);
    }
}
