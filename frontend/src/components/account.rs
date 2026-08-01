use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::UserInfo;

use crate::api;
use crate::auth::use_auth;
use crate::dropdown::dismiss_on_outside_click;

/// What to call the user on screen. `display_name` is optional and may have
/// been blanked out by a profile edit, so the username is the fallback — it is
/// required and unique, so there is always something to show.
fn display_name(user: &UserInfo) -> String {
    user.display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(&user.username)
        .to_string()
}

/// Single character for the fallback avatar. Skips leading punctuation so a
/// name like `_sher` still shows an `S`.
fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into())
}

const AVATAR_IMG: &str = "object-cover rounded-full size-8 shrink-0 bg-base-300";
const AVATAR_INITIAL: &str = "flex justify-center items-center text-sm font-semibold rounded-full size-8 shrink-0 bg-primary text-primary-content";

#[component]
fn Avatar(user: UserInfo) -> impl IntoView {
    let letter = initial(&display_name(&user));
    let url = user
        .avatar_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(String::from);

    let Some(url) = url else {
        return view! { <span class=AVATAR_INITIAL>{letter}</span> }.into_any();
    };

    // A stored avatar_url can 404, expire, or point at something that is not an
    // image — the browser paints its broken-image glyph for all three. Fall
    // back to the initial instead. A fresh `Avatar` is built whenever the user
    // changes, so this resets itself when the avatar does.
    let (broken, set_broken) = signal(false);
    let fallback_letter = letter.clone();
    view! {
        <Show
            when=move || !broken.get()
            fallback=move || {
                view! { <span class=AVATAR_INITIAL>{fallback_letter.clone()}</span> }
            }
        >
            <img src=url.clone() alt="" class=AVATAR_IMG on:error=move |_| set_broken.set(true) />
        </Show>
    }
    .into_any()
}

/// Signed out: a sign-in link. Signed in: the display name and avatar, which
/// open a menu holding sign-out.
#[component]
pub fn AccountMenu() -> impl IntoView {
    let auth = use_auth();
    let (open, set_open) = signal(false);
    let root: NodeRef<leptos::html::Div> = NodeRef::new();
    dismiss_on_outside_click(root, set_open);

    let sign_out = move |_| {
        set_open.set(false);
        spawn_local(async move {
            let _ = api::logout().await;
            auth.user.set(None);
            if let Some(window) = web_sys::window() {
                let _ = window.location().set_hash("/");
            }
        });
    };

    view! {
        <div class="flex relative items-center" node_ref=root>
            <Show
                when=move || auth.user.get().is_some()
                fallback=|| {
                    view! {
                        // Plain `btn`, not `btn-sm`: every control in the
                        // navbar shares the same height.
                        <a href="#/login" class="gap-2 btn btn-primary">
                            <span class="icon-[tabler--login] size-5"></span>
                            "Sign in"
                        </a>
                    }
                }
            >
                <button
                    class="flex gap-2 items-center rounded-full btn btn-text"
                    aria-haspopup="menu"
                    aria-expanded=move || open.get().to_string()
                    aria-label="Account menu"
                    on:click=move |_| set_open.update(|o| *o = !*o)
                >
                    <span class="hidden text-sm sm:inline">
                        {move || auth.user.get().map(|u| display_name(&u)).unwrap_or_default()}
                    </span>
                    {move || auth.user.get().map(|user| view! { <Avatar user=user /> })}
                </button>

                <Show when=move || open.get()>
                    <ul
                        class="absolute right-0 top-full z-10 p-2 mt-2 w-52 rounded-lg border shadow-lg bg-base-100 border-base-content/10 motion-preset-slide-down motion-duration-200"
                        role="menu"
                    >
                        <li class="py-2 px-3 border-b border-base-content/10">
                            <p class="font-semibold truncate">
                                {move || {
                                    auth.user.get().map(|u| display_name(&u)).unwrap_or_default()
                                }}
                            </p>
                            <p class="text-sm truncate text-base-content/60">
                                {move || auth.user.get().map(|u| u.username).unwrap_or_default()}
                            </p>
                        </li>
                        <li class="mt-1">
                            <button
                                class="flex gap-2 items-center py-2 px-3 w-full text-left rounded-md hover:bg-base-200"
                                role="menuitem"
                                on:click=sign_out
                            >
                                <span class="icon-[tabler--logout] size-5"></span>
                                "Sign out"
                            </button>
                        </li>
                    </ul>
                </Show>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(display: Option<&str>, avatar: Option<&str>) -> UserInfo {
        UserInfo {
            id: "u1".into(),
            username: "sher".into(),
            display_name: display.map(String::from),
            email: None,
            avatar_url: avatar.map(String::from),
            is_admin: false,
            has_password: true,
        }
    }

    #[test]
    fn display_name_falls_back_to_the_username() {
        assert_eq!(display_name(&user(Some("Sher Ly"), None)), "Sher Ly");
        assert_eq!(display_name(&user(None, None)), "sher");
        // A profile edit can leave this blank; blank must not render as empty.
        assert_eq!(display_name(&user(Some("   "), None)), "sher");
    }

    #[test]
    fn initial_skips_leading_punctuation() {
        assert_eq!(initial("Sher Ly"), "S");
        assert_eq!(initial("_sher"), "S");
        assert_eq!(initial("9lives"), "9");
        assert_eq!(initial("___"), "?");
        assert_eq!(initial(""), "?");
    }
}
