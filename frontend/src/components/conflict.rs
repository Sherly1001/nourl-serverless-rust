use leptos::prelude::*;
use shared::{OwnerInfo, UrlEntry};

use crate::auth::AuthContext;
use crate::components::avatar::Avatar;
use crate::components::tooltip::Tooltip;
use crate::datetime::{from_rfc3339, local_offset_minutes, to_display};

/// The link's owner, when that is somebody other than whoever is being asked
/// about it. `None` covers all three of "nobody owns it", "you do", and "not
/// signed in" — none of which is a fact worth putting in front of the caller.
pub fn other_owner(existing: &UrlEntry, auth: &AuthContext) -> Option<String> {
    let name = existing.owner.as_ref()?.username.clone()?;
    let me = auth.user.get().map(|u| u.username);
    (me.as_deref() != Some(name.as_str())).then_some(name)
}

fn shown_expiry(stamp: Option<&String>) -> String {
    stamp
        .and_then(|stamp| from_rfc3339(stamp, local_offset_minutes()))
        .and_then(|local| to_display(&local))
        .unwrap_or_else(|| "never".into())
}

/// One fact about the link being replaced, and what it becomes.
///
/// Stacked rather than side by side: a destination is the thing being judged
/// here, and two of them sharing a line leaves neither enough room to be read.
/// Each still truncates — a dialog that grows to fit a tracking-parameter tail
/// pushes its own buttons off the screen.
#[component]
fn Change(label: &'static str, was: String, now: String) -> impl IntoView {
    let changed = was != now;
    let (was, now) = (StoredValue::new(was), StoredValue::new(now));
    view! {
        <div class="grid gap-2 items-baseline grid-cols-[5rem_1fr]">
            <span class="text-xs uppercase text-base-content/50">{label}</span>
            <span class="min-w-0">
                <Tooltip
                    text=was.get_value()
                    class=if changed {
                        "block min-w-0 truncate line-through text-base-content/50"
                    } else {
                        "block min-w-0 truncate"
                    }
                >
                    {was.get_value()}
                </Tooltip>
                <Show when=move || changed>
                    <span class="flex gap-2 items-baseline min-w-0">
                        <span class="text-base-content/40">"→"</span>
                        <Tooltip text=now.get_value() class="block min-w-0 font-medium truncate">
                            {now.get_value()}
                        </Tooltip>
                    </span>
                </Show>
            </span>
        </div>
    }
}

/// What replacing a link would change, and the one or two questions that cannot
/// be answered for the caller.
///
/// The hit count is left alone by default: pointing a link somewhere else does
/// not on its own mean the visits it already had stop counting, and only the
/// person replacing it knows whether this is the same link with a new target or
/// a different link wearing the same code.
///
/// Ownership likewise. An admin may write over anybody's link, and fixing a
/// broken destination should not quietly acquire it — so taking it is a
/// separate tick, offered only when the link is somebody else's and is going to
/// survive this write.
#[component]
pub fn ReplacementDetails(
    existing: UrlEntry,
    #[prop(into)] url: Signal<String>,
    /// The new expiry as the field shows it, empty for none.
    #[prop(into)]
    expires: Signal<String>,
    reset_hits: RwSignal<bool>,
    /// Absent when the link is about to be deleted rather than replaced: there
    /// is nothing left to take.
    #[prop(default = None)]
    claim: Option<RwSignal<bool>>,
    /// Whose the link is, when that is not the person being asked.
    #[prop(default = None)]
    other_owner: Option<OwnerInfo>,
) -> impl IntoView {
    let was_url = existing.url.clone();
    let was_expiry = shown_expiry(existing.expires_at.as_ref());
    let hits = existing.hits;
    let owner_name = StoredValue::new(
        other_owner
            .as_ref()
            .and_then(|owner| owner.username.clone())
            .unwrap_or_default(),
    );
    let owner_avatar = other_owner
        .as_ref()
        .and_then(|owner| owner.avatar_url.clone());

    view! {
        <div class="p-3 mb-6 space-y-2 text-sm rounded-lg bg-base-200">
            <Show when=move || owner_name.with_value(|name| !name.is_empty())>
                <div class="grid gap-2 items-center grid-cols-[5rem_1fr]">
                    <span class="text-xs uppercase text-base-content/50">"Owner"</span>
                    <span class="flex gap-2 items-center min-w-0">
                        <Avatar
                            url=owner_avatar.clone()
                            name=owner_name.get_value()
                            size="size-5"
                        />
                        <Tooltip text=owner_name.get_value() class="block min-w-0 truncate">
                            {owner_name.get_value()}
                        </Tooltip>
                    </span>
                </div>
            </Show>
            <Change label="Goes to" was=was_url now=url.get_untracked() />
            <Change
                label="Expires"
                was=was_expiry
                now=if expires.get_untracked().trim().is_empty() {
                    "never".into()
                } else {
                    expires.get_untracked()
                }
            />
        </div>

        <label class="flex gap-3 items-center mb-3 cursor-pointer">
            <input
                type="checkbox"
                class="checkbox checkbox-sm"
                prop:checked=move || reset_hits.get()
                on:change=move |_| reset_hits.update(|on| *on = !*on)
            />
            <span class="text-sm">{format!("Reset the hit count ({hits} so far)")}</span>
        </label>

        {claim
            .map(|claim| {
                view! {
                    <label class="flex gap-3 items-center mb-3 cursor-pointer">
                        <input
                            type="checkbox"
                            class="checkbox checkbox-sm"
                            prop:checked=move || claim.get()
                            on:change=move |_| claim.update(|on| *on = !*on)
                        />
                        <span class="text-sm">"Take ownership of this link"</span>
                    </label>
                }
            })}
        <div class="mb-3"></div>
    }
}
