use leptos::prelude::*;
use wasm_bindgen::JsCast;

const STORAGE_KEY: &str = "nourl-theme";

/// Sentinel stored when the user wants to follow the OS preference.
const SYSTEM: &str = "system";

/// Every theme registered in `style/input.css`, in menu order.
pub const THEMES: &[&str] = &[
    "light",
    "dark",
    "black",
    "claude",
    "corporate",
    "ghibli",
    "gourmet",
    "luxury",
    "mintlify",
    "pastel",
    "perplexity",
    "shadcn",
    "slack",
    "soft",
    "spotify",
    "valorant",
    "vscode",
];

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn system_prefers_dark() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
        .is_some_and(|list| list.matches())
}

/// Stored selection, or `SYSTEM` when the user has never chosen.
fn stored_selection() -> String {
    storage()
        .and_then(|s| s.get_item(STORAGE_KEY).ok().flatten())
        .filter(|v| v == SYSTEM || THEMES.contains(&v.as_str()))
        .unwrap_or_else(|| SYSTEM.to_string())
}

/// Theme actually applied to the document for a given selection.
fn resolve(selection: &str) -> &str {
    if selection == SYSTEM {
        if system_prefers_dark() {
            "dark"
        } else {
            "light"
        }
    } else {
        selection
    }
}

fn apply(selection: &str) {
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = root.set_attribute("data-theme", resolve(selection));
    }
}

#[component]
pub fn ThemePicker() -> impl IntoView {
    let (selection, set_selection) = signal(stored_selection());
    let (open, set_open) = signal(false);
    Effect::new(move |_| apply(&selection.get()));

    let choose = move |value: &'static str| {
        if let Some(storage) = storage() {
            let _ = storage.set_item(STORAGE_KEY, value);
        }
        set_selection.set(value.to_string());
        set_open.set(false);
    };

    view! {
        <div class="relative">
            <button
                class="gap-2 btn btn-text"
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| set_open.update(|o| *o = !*o)
            >
                <span class="icon-[tabler--palette] size-5"></span>
                <span class="hidden capitalize sm:inline">{move || selection.get()}</span>
                <span class="opacity-60 icon-[tabler--chevron-down] size-4"></span>
            </button>

            <Show when=move || open.get()>
                <ul
                    class="overflow-y-auto absolute right-0 z-10 p-2 mt-2 space-y-1 w-48 max-h-80 rounded-lg border shadow-lg bg-base-100 border-base-content/10"
                    role="menu"
                >
                    <li>
                        <button
                            class="py-2 px-3 w-full text-left rounded-md border border-transparent hover:bg-base-200"
                            class:bg-base-200=move || selection.get() == SYSTEM
                            on:click=move |_| choose(SYSTEM)
                        >
                            "System"
                        </button>
                    </li>
                    {THEMES
                        .iter()
                        .map(|name| {
                            view! {
                                <li>
                                    <button
                                        class="flex gap-2 items-center py-2 px-3 w-full text-left capitalize rounded-md border transition-colors bg-base-100 text-base-content border-base-content/10 hover:border-primary"
                                        class:ring-2=move || selection.get() == *name
                                        class:ring-primary=move || selection.get() == *name
                                        data-theme=*name
                                        on:click=move |_| choose(name)
                                    >
                                        <span class="rounded-full bg-primary size-3"></span>
                                        {*name}
                                    </button>
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            </Show>
        </div>
    }
}
