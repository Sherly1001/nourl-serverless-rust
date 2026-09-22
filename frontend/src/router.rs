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

/// A query string names no page: the route is whatever precedes the `?`.
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

pub fn route_path(hash: &str) -> String {
    hash.split('?').next().unwrap_or(hash).to_string()
}

/// Unreserved by RFC 3986; everything else is escaped, since a destination
/// filter carries `:` and `/` and a search carries spaces.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(&value[i + 1..i + 3], 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    // A stray `%` is a character, not a broken escape.
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every pair after the `?`, in order, so an item repeated for each of its
/// values survives the trip.
pub fn query_pairs(hash: &str) -> Vec<(String, String)> {
    let Some((_, query)) = hash.split_once('?') else {
        return Vec::new();
    };
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            let (key, value) = (decode(key), decode(value));
            (!key.is_empty() && !value.trim().is_empty()).then_some((key, value))
        })
        .collect()
}

/// The reverse. No pairs leaves no `?`, which would be noise in the address bar.
pub fn with_query(path: &str, pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return path.to_string();
    }
    let query: Vec<String> = pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect();
    format!("{path}?{}", query.join("&"))
}

/// Writes the query of the page already shown. `replaceState` rather than a
/// push: Back should leave the page, not walk back through every filter change.
/// It fires no `hashchange`, so this cannot loop through its own listener.
pub fn replace_query(pairs: &[(String, String)]) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let path = route_path(&read_hash());
    let next = with_query(&path, pairs);
    if next == read_hash() {
        return;
    }
    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&next));
    }
}

/// The current hash's query, refreshed whenever the hash changes.
pub fn use_hash_query() -> Signal<Vec<(String, String)>> {
    let (hash, set_hash) = signal(read_hash());
    let closure = Closure::<dyn FnMut()>::new(move || set_hash.set(read_hash()));
    if let Some(window) = web_sys::window() {
        let _ =
            window.add_event_listener_with_callback("hashchange", closure.as_ref().unchecked_ref());
    }
    closure.forget();
    Signal::derive(move || query_pairs(&hash.get()))
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

    #[test]
    fn a_query_round_trips_through_the_hash() {
        let pairs = vec![
            ("owner".to_string(), "ann".to_string()),
            ("owner".to_string(), "bob".to_string()),
            ("url".to_string(), "https://a.example/x y".to_string()),
            ("q".to_string(), "a&b=c".to_string()),
        ];
        let hash = with_query("#/urls", &pairs);
        assert!(hash.starts_with("#/urls?"));
        assert_eq!(query_pairs(&hash), pairs);
        assert_eq!(with_query("#/urls", &[]), "#/urls");
    }

    #[test]
    fn a_hand_edited_query_keeps_only_what_reads() {
        let pairs = query_pairs("#/urls?owner=ann&broken&empty=&=x&q=%20%41");
        assert_eq!(
            pairs,
            vec![
                ("owner".to_string(), "ann".to_string()),
                ("q".to_string(), " A".to_string()),
            ]
        );
    }

    #[test]
    fn the_path_is_whatever_precedes_the_query() {
        assert_eq!(route_path("#/urls?owner=ann"), "#/urls");
        assert_eq!(route_path("#/urls"), "#/urls");
    }

    /// Without this, `#/login?error=code` matches nothing and the page that
    /// could explain the failure is replaced by a 404.
    #[test]
    fn a_query_string_does_not_change_which_page_is_shown() {
        assert_eq!(parse_route("#/login?error=oauth_denied"), Route::Login);
        assert_eq!(parse_route("#/urls?page=2"), Route::MyUrls);
        assert_eq!(parse_route("#/?x=1"), Route::Shorten);
        assert_eq!(parse_route("#/nope?error=oauth_denied"), Route::NotFound);
    }
}
