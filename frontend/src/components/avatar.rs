use leptos::prelude::*;

/// Single character for the fallback avatar. Skips leading punctuation so a
/// name like `_sher` still shows an `S`.
pub fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into())
}

/// Trims a stored url and treats blank as absent, so an emptied profile field
/// does not render a broken image.
pub fn usable_url(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|url| !url.is_empty())
        .map(String::from)
}

/// A picture, or the name's first letter. `size` is the Tailwind class, so
/// both shapes stay round at any scale.
#[component]
pub fn Avatar(
    /// Already-trimmed image url, or `None` for the lettered fallback.
    url: Option<String>,
    /// Name the initial is taken from.
    name: String,
    #[prop(default = "size-8")] size: &'static str,
) -> impl IntoView {
    let letter = initial(&name);
    let img_class = format!("object-cover rounded-full shrink-0 bg-base-300 {size}");
    let initial_class = format!(
        "flex justify-center items-center text-sm font-semibold rounded-full shrink-0 bg-primary text-primary-content {size}"
    );

    let Some(url) = url else {
        return view! { <span class=initial_class>{letter}</span> }.into_any();
    };

    // A stored url can 404, expire, or point at something that is not an image
    // — the browser paints its broken-image glyph for all three. Fall back to
    // the initial instead. A fresh `Avatar` is built whenever its inputs
    // change, so this resets itself with a new picture.
    let (broken, set_broken) = signal(false);
    let fallback_class = initial_class.clone();
    view! {
        <Show
            when=move || !broken.get()
            fallback={
                let letter = letter.clone();
                let class = fallback_class.clone();
                move || view! { <span class=class.clone()>{letter.clone()}</span> }
            }
        >
            <img
                src=url.clone()
                alt=""
                class=img_class.clone()
                on:error=move |_| set_broken.set(true)
            />
        </Show>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_skips_leading_punctuation() {
        assert_eq!(initial("Sher Ly"), "S");
        assert_eq!(initial("_sher"), "S");
        assert_eq!(initial("9lives"), "9");
        assert_eq!(initial("___"), "?");
        assert_eq!(initial(""), "?");
    }

    #[test]
    fn a_blank_url_counts_as_no_picture() {
        assert_eq!(usable_url(None), None);
        assert_eq!(usable_url(Some("   ")), None);
        assert_eq!(
            usable_url(Some("  https://x.io/a.png ")),
            Some("https://x.io/a.png".into())
        );
    }
}
