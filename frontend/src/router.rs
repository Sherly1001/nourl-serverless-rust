use leptos::prelude::*;
use wasm_bindgen::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Route {
    Shorten,
    Login,
    Account,
    MyUrls,
    Users,
    Settings,
    NotFound,
}

fn read_hash() -> String {
    web_sys::window()
        .and_then(|w| w.location().hash().ok())
        .unwrap_or_default()
}

/// A query string names no page. The OAuth callback carries its failure back
/// as `#/login?error=code`, and pages are free to keep state there, so the
/// route is whatever comes before the `?`.
pub fn parse_route(hash: &str) -> Route {
    let path = hash.split('?').next().unwrap_or(hash);
    match path.trim_start_matches('#').trim_start_matches('/') {
        "" => Route::Shorten,
        "login" => Route::Login,
        "account" => Route::Account,
        "urls" => Route::MyUrls,
        "users" => Route::Users,
        "settings" => Route::Settings,
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
    fn parses_the_account_route() {
        assert_eq!(parse_route("#/account"), Route::Account);
    }

    #[test]
    fn parses_the_admin_routes() {
        assert_eq!(parse_route("#/users"), Route::Users);
        assert_eq!(parse_route("#/settings"), Route::Settings);
    }

    #[test]
    fn parses_the_authenticated_routes() {
        assert_eq!(parse_route("#/login"), Route::Login);
        assert_eq!(parse_route("#/urls"), Route::MyUrls);
        assert_eq!(parse_route("#/nope"), Route::NotFound);
    }

    /// The OAuth callback redirects to `#/login?error=code`. Without this the
    /// route is read as `login?error=code`, matches nothing, and the one page
    /// that knows how to explain the failure is replaced by a 404.
    #[test]
    fn a_query_string_does_not_change_which_page_is_shown() {
        assert_eq!(parse_route("#/login?error=oauth_denied"), Route::Login);
        assert_eq!(parse_route("#/urls?page=2"), Route::MyUrls);
        assert_eq!(parse_route("#/?x=1"), Route::Shorten);
        assert_eq!(parse_route("#/nope?error=oauth_denied"), Route::NotFound);
    }
}
