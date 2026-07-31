use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

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

    // Dismiss on a click anywhere outside the picker, or on Escape. Listeners
    // sit on the document because clicks elsewhere never reach this subtree.
    let root: NodeRef<leptos::html::Div> = NodeRef::new();
    let pointer = Closure::<dyn FnMut(web_sys::Event)>::new(move |ev: web_sys::Event| {
        let Some(container) = root.get_untracked() else {
            return;
        };
        let inside = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
            .is_some_and(|node| container.contains(Some(&node)));
        if !inside {
            set_open.set(false);
        }
    });
    let keydown =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Escape" {
                set_open.set(false);
            }
        });
    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
        let _ = document
            .add_event_listener_with_callback("pointerdown", pointer.as_ref().unchecked_ref());
        let _ =
            document.add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref());
    }
    pointer.forget();
    keydown.forget();

    view! {
        <div class="relative" node_ref=root>
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
                    class="overflow-y-auto absolute right-0 z-10 p-2 mt-2 space-y-1 w-48 max-h-80 rounded-lg border shadow-lg bg-base-100 border-base-content/10 motion-preset-slide-down motion-duration-200"
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
