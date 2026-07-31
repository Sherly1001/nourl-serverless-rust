use leptos::prelude::*;

use crate::theme::ThemePicker;

#[component]
pub fn App() -> impl IntoView {
    view! {
        <div class="shadow navbar bg-base-200">
            <div class="navbar-start">
                <a href="#/" class="text-xl font-bold">
                    "NoUrl"
                </a>
            </div>
            <div class="navbar-end">
                <ThemePicker />
            </div>
        </div>
        <main class="container p-4 mx-auto max-w-xl">
            <p>"loading…"</p>
        </main>
    }
}
