use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::{UrlEntry, UrlUpsertRequest, validate_code, validate_url};
use wasm_bindgen_futures::JsFuture;

use crate::api;
use crate::ui::input_class;

fn origin() -> String {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default()
}

fn copy_to_clipboard(text: String) {
    if let Some(window) = web_sys::window() {
        let clipboard = window.navigator().clipboard();
        spawn_local(async move {
            let _ = JsFuture::from(clipboard.write_text(&text)).await;
        });
    }
}

fn check_code(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        return Some("Code is required".into());
    }
    validate_code(value).err()
}

fn check_url(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        return Some("Destination URL is required".into());
    }
    validate_url(value).err()
}

#[component]
pub fn Shorten() -> impl IntoView {
    let (code, set_code) = signal(String::new());
    let (url, set_url) = signal(String::new());
    // A field reports problems only once the user has typed in it, blurred it,
    // or tried to submit — so the form does not open covered in red.
    let (code_touched, set_code_touched) = signal(false);
    let (url_touched, set_url_touched) = signal(false);
    let (code_server_error, set_code_server_error) = signal(Option::<String>::None);
    let (result, set_result) = signal(Option::<UrlEntry>::None);
    let (busy, set_busy) = signal(false);
    let (copied, set_copied) = signal(false);

    let code_error = Memo::new(move |_| {
        code_server_error.get().or_else(|| {
            code_touched
                .get()
                .then(|| check_code(&code.get()))
                .flatten()
        })
    });
    let url_error = Memo::new(move |_| url_touched.get().then(|| check_url(&url.get())).flatten());

    // Submitting is pointless while a field is empty or showing an error.
    let can_submit = Memo::new(move |_| {
        !busy.get()
            && code_error.get().is_none()
            && url_error.get().is_none()
            && check_code(&code.get()).is_none()
            && check_url(&url.get()).is_none()
    });

    let reset = move || {
        set_code.set(String::new());
        set_url.set(String::new());
        set_code_touched.set(false);
        set_url_touched.set(false);
        set_code_server_error.set(None);
        set_result.set(None);
        set_copied.set(false);
    };

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        set_code_touched.set(true);
        set_url_touched.set(true);

        let code_value = code.get();
        let url_value = url.get();
        if check_code(&code_value).is_some() || check_url(&url_value).is_some() {
            return;
        }

        set_busy.set(true);
        spawn_local(async move {
            let request = UrlUpsertRequest {
                code: code_value,
                url: url_value,
                expires_at: None,
            };
            match api::create_url(&request).await {
                Ok(entry) => set_result.set(Some(entry)),
                // Server-side rejections are always about the code (taken,
                // reserved, malformed), so surface them on that field.
                Err(err) => set_code_server_error.set(Some(err.message)),
            }
            set_busy.set(false);
        });
    };

    let code_input: NodeRef<leptos::html::Input> = NodeRef::new();
    let focus_code = move || {
        if let Some(input) = code_input.get_untracked() {
            let _ = input.focus();
        }
    };
    Effect::new(move |_| {
        if let Some(input) = code_input.get() {
            let _ = input.focus();
        }
    });

    // Once the link exists the form is a record of what was created, not an
    // editable draft: the only way on is "Create another short link", which
    // resets it. Leaving the fields live would invite edits that go nowhere.
    let done = Memo::new(move |_| result.get().is_some());

    view! {
        <div class="border shadow-xl card bg-base-200 border-base-content/10 motion-preset-fade motion-duration-500">
            <div class="gap-6 p-8 card-body">
                <h2 class="text-3xl card-title">"Shorten a URL"</h2>

                <form class="flex flex-col gap-3" novalidate on:submit=submit>
                    <div class="w-full">
                        <label class="mb-2 text-lg font-semibold label-text" for="code">
                            "Code"
                        </label>
                        <input
                            id="code"
                            node_ref=code_input
                            class=move || input_class(code_error.get().is_some())
                            placeholder="my-code"
                            autocomplete="off"
                            aria-invalid=move || code_error.get().is_some().to_string()
                            aria-describedby="code-error"
                            disabled=move || done.get()
                            prop:value=code
                            on:input=move |ev| {
                                set_code.set(event_target_value(&ev));
                                set_code_touched.set(true);
                                set_code_server_error.set(None);
                            }
                            on:blur=move |_| set_code_touched.set(true)
                        />
                        <p
                            id="code-error"
                            class="mt-1 text-base min-h-6 text-error motion-preset-slide-down motion-duration-200"
                        >
                            {move || code_error.get().unwrap_or_default()}
                        </p>
                    </div>

                    <div class="w-full">
                        <label class="mb-2 text-lg font-semibold label-text" for="url">
                            "Destination URL"
                        </label>
                        <input
                            id="url"
                            class=move || input_class(url_error.get().is_some())
                            placeholder="https://example.com"
                            autocomplete="off"
                            aria-invalid=move || url_error.get().is_some().to_string()
                            aria-describedby="url-error"
                            disabled=move || done.get()
                            prop:value=url
                            on:input=move |ev| {
                                set_url.set(event_target_value(&ev));
                                set_url_touched.set(true);
                            }
                            on:blur=move |_| set_url_touched.set(true)
                        />
                        <p
                            id="url-error"
                            class="mt-1 text-base min-h-6 text-error motion-preset-slide-down motion-duration-200"
                        >
                            {move || url_error.get().unwrap_or_default()}
                        </p>
                    </div>

                    <Show
                        when=move || result.get().is_some()
                        fallback=move || {
                            view! {
                                <button
                                    class="h-14 text-xl font-semibold btn btn-primary active:scale-[.98]"
                                    type="submit"
                                    disabled=move || !can_submit.get()
                                >
                                    <Show when=move || busy.get()>
                                        <span class="loading loading-spinner loading-sm"></span>
                                    </Show>
                                    {move || if busy.get() { "Saving…" } else { "Shorten" }}
                                </button>
                            }
                        }
                    >
                        <button
                            class="h-14 text-xl font-semibold btn btn-outline btn-primary active:scale-[.98]"
                            type="button"
                            on:click=move |_| {
                                reset();
                                focus_code();
                            }
                        >
                            <span class="icon-[tabler--plus] size-5"></span>
                            "Create another short link"
                        </button>
                    </Show>
                </form>

                {move || {
                    result
                        .get()
                        .map(|entry| {
                            let short = format!("{}/{}", origin(), entry.code);
                            let href = short.clone();
                            let to_copy = short.clone();
                            view! {
                                <div class="flex gap-3 justify-between items-center alert alert-success motion-preset-slide-up motion-duration-300">
                                    <span class="icon-[tabler--circle-check] size-5 shrink-0"></span>
                                    <a
                                        href=href
                                        target="_blank"
                                        rel="noreferrer"
                                        class="flex-1 font-mono truncate link text-success-content hover:text-success-content focus-visible:text-success-content"
                                    >
                                        {short}
                                    </a>
                                    <button
                                        class="active:scale-95 btn btn-sm btn-outline shrink-0 text-success-content border-success-content/40 hover:bg-success-content/10"
                                        on:click=move |_| {
                                            copy_to_clipboard(to_copy.clone());
                                            set_copied.set(true);
                                        }
                                    >
                                        <span class="icon-[tabler--copy] size-4"></span>
                                        {move || if copied.get() { "Copied" } else { "Copy" }}
                                    </button>
                                </div>
                            }
                        })
                }}
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_is_reported_before_format_rules() {
        assert_eq!(check_code("").as_deref(), Some("Code is required"));
        assert_eq!(check_code("   ").as_deref(), Some("Code is required"));
        assert_eq!(
            check_url("").as_deref(),
            Some("Destination URL is required")
        );
    }

    #[test]
    fn format_errors_come_from_shared_validators() {
        assert!(check_code("bad/code").is_some());
        assert!(check_url("ftp://x.com").is_some());
    }

    #[test]
    fn valid_input_has_no_error() {
        assert!(check_code("my-code").is_none());
        assert!(check_url("https://example.com").is_none());
    }
}
