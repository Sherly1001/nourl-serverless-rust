use leptos::prelude::*;

use crate::auth::provide_auth;
use crate::components::account::AccountMenu;
use crate::components::account_page::AccountPage;
use crate::components::login::Login;
use crate::components::my_urls::MyUrls;
use crate::components::settings::Settings;
use crate::components::shorten::Shorten;
use crate::components::toasts::ToastStack;
use crate::components::users::Users;
use crate::router::{Route, use_hash_route};
use crate::theme::ThemePicker;
use crate::toast::provide_toasts;

#[component]
pub fn App() -> impl IntoView {
    let route = use_hash_route();
    let auth = provide_auth();
    provide_toasts();

    // Which tab is showing, marked on the link that leads there. `btn-soft`
    // rather than a colour swap: it is the same button, lit, so the row still
    // reads as one set of tabs.
    let tab = move |target: Route| {
        if route.get() == target {
            "gap-2 btn btn-soft btn-primary"
        } else {
            "gap-2 btn btn-text"
        }
    };
    let current = move |target: Route| {
        if route.get() == target {
            "page"
        } else {
            "false"
        }
    };

    view! {
        // Sticky, and below the row menus (`z-30`) rather than above them: a
        // header that paints over an open dropdown is worse than one a
        // dropdown briefly covers. The page's own sticky table heads sit at
        // `z-10`, so those still scroll under it.
        <div class="sticky top-0 z-20 border-b shadow-sm navbar bg-base-200 border-base-content/10">
            <div class="navbar-start">
                <a href="#/" class="flex gap-2 items-center text-xl font-bold">
                    <img src="/favicon.png" alt="" class="size-7" />
                    "NoUrl"
                </a>
            </div>
            <div class="flex gap-2 items-center navbar-end">
                <Show when=move || auth.user.get().is_some()>
                    <a
                        href="#/urls"
                        class=move || tab(Route::MyUrls)
                        aria-current=move || current(Route::MyUrls)
                    >
                        <span class="icon-[tabler--link] size-5"></span>
                        "My URLs"
                    </a>
                </Show>
                <Show when=move || auth.is_admin()>
                    <a
                        href="#/users"
                        class=move || tab(Route::Users)
                        aria-current=move || current(Route::Users)
                    >
                        <span class="icon-[tabler--users] size-5"></span>
                        "Users"
                    </a>
                </Show>
                // The sign-in settings belong to the root alone, so an ordinary
                // admin must not be offered a link that only ends in a 403.
                <Show when=move || auth.is_root()>
                    <a
                        href="#/settings"
                        class=move || tab(Route::Settings)
                        aria-current=move || current(Route::Settings)
                    >
                        <span class="icon-[tabler--settings] size-5"></span>
                        "Settings"
                    </a>
                </Show>
                <ThemePicker />
                <AccountMenu />
            </div>
        </div>

        // The table needs room; a form reads better narrow, so the width
        // follows the route rather than being one compromise for both. The URL
        // table is the widest of them — nine columns and an action pair — and
        // anything narrower than this scrolls the actions out of reach.
        <main class=move || {
            let width = match route.get() {
                Route::MyUrls => "max-w-[86rem]",
                Route::Users => "max-w-6xl",
                _ => "max-w-2xl",
            };
            format!("container py-10 px-4 mx-auto {width}")
        }>
            {move || match route.get() {
                Route::Shorten => view! { <Shorten /> }.into_any(),
                Route::Login => view! { <Login /> }.into_any(),
                Route::Account => view! { <AccountPage /> }.into_any(),
                Route::MyUrls => view! { <MyUrls /> }.into_any(),
                Route::Users => view! { <Users /> }.into_any(),
                Route::Settings => view! { <Settings /> }.into_any(),
                Route::NotFound => view! { <NotFound /> }.into_any(),
            }}
        </main>

        <ToastStack />
    }
}

#[component]
fn NotFound() -> impl IntoView {
    view! {
        <div class="flex flex-col gap-6 items-center py-16 text-center motion-preset-fade motion-duration-500">
            <span class="icon-[tabler--link-off] size-20 text-base-content/25"></span>

            <div class="space-y-2">
                <p class="font-mono text-6xl font-bold text-base-content/20">"404"</p>
                <h1 class="text-2xl font-semibold">"This page does not exist"</h1>
                <p class="text-base-content/60">
                    "The link you followed points nowhere. Short links live at the
                    root of this site, not behind #."
                </p>
            </div>

            <a href="#/" class="gap-2 h-12 text-lg btn btn-primary active:scale-[.98]">
                <span class="icon-[tabler--arrow-left] size-5"></span>
                "Back to shortening"
            </a>
        </div>
    }
}
