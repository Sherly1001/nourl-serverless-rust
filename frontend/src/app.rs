use leptos::prelude::*;

use crate::components::shorten::Shorten;
use crate::router::{Route, use_hash_route};
use crate::theme::ThemePicker;

#[component]
pub fn App() -> impl IntoView {
    let route = use_hash_route();

    view! {
        <div class="border-b shadow-sm navbar bg-base-200 border-base-content/10">
            <div class="navbar-start">
                <a href="#/" class="flex gap-2 items-center text-xl font-bold">
                    <img src="/favicon.png" alt="" class="size-7" />
                    "NoUrl"
                </a>
            </div>
            <div class="navbar-end">
                <ThemePicker />
            </div>
        </div>

        <main class="container py-10 px-4 mx-auto max-w-2xl">
            {move || match route.get() {
                Route::Shorten => view! { <Shorten /> }.into_any(),
                Route::NotFound => view! { <NotFound /> }.into_any(),
            }}
        </main>
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
