use leptos::prelude::*;

/// How long a toast stays up, by severity. An error gets much longer because
/// it usually carries something the reader has to act on.
pub const SUCCESS_MS: u32 = 3_000;
pub const ERROR_MS: u32 = 10_000;

/// Height one card occupies in the stack, including the gap below it. Used to
/// work out how many fit rather than picking a number out of the air.
const CARD_SLOT_PX: f64 = 88.0;
/// Navbar, top offset and a little breathing room at the bottom.
const STACK_CHROME_PX: f64 = 140.0;
/// Assumed viewport when there is no window to measure — the tests, and any
/// call before the page is laid out.
const ASSUMED_VIEWPORT_PX: f64 = 800.0;

/// How many toasts fit beside the page at this viewport height. Always at
/// least one, so a message can never be dropped before it is shown.
pub fn capacity_for(viewport_px: f64) -> usize {
    let usable = viewport_px - STACK_CHROME_PX;
    ((usable / CARD_SLOT_PX).floor().max(1.0)) as usize
}

fn viewport_height() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.inner_height().ok())
            .and_then(|value| value.as_f64())
            .unwrap_or(ASSUMED_VIEWPORT_PX)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        ASSUMED_VIEWPORT_PX
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Warning,
    Error,
}

impl ToastKind {
    pub fn lifetime_ms(self) -> u32 {
        match self {
            ToastKind::Error => ERROR_MS,
            _ => SUCCESS_MS,
        }
    }

    /// FlyonUI alert modifier.
    pub fn alert_class(self) -> &'static str {
        match self {
            ToastKind::Success => "alert-success",
            ToastKind::Warning => "alert-warning",
            ToastKind::Error => "alert-error",
        }
    }

    pub fn icon_class(self) -> &'static str {
        match self {
            ToastKind::Success => "icon-[tabler--circle-check]",
            ToastKind::Warning => "icon-[tabler--alert-triangle]",
            ToastKind::Error => "icon-[tabler--alert-circle]",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub id: u32,
    pub kind: ToastKind,
    pub message: String,
}

/// Shared queue of toasts. Anything anywhere in the app can push one without
/// owning a place to render it.
#[derive(Clone, Copy)]
pub struct Toasts {
    items: RwSignal<Vec<Toast>>,
    next_id: RwSignal<u32>,
}

impl Toasts {
    pub fn items(&self) -> Vec<Toast> {
        self.items.get()
    }

    pub fn push(&self, kind: ToastKind, message: impl Into<String>) {
        let id = self.next_id.get_untracked();
        self.next_id.set(id.wrapping_add(1));
        let capacity = capacity_for(viewport_height());
        self.items.update(|list| {
            // Full stack: the oldest goes. It has been readable the longest and
            // is nearest its own expiry, and dropping it keeps every remaining
            // card at full height instead of squashing the pile.
            while list.len() >= capacity {
                list.remove(0);
            }
            list.push(Toast {
                id,
                kind,
                message: message.into(),
            })
        });
    }

    pub fn success(&self, message: impl Into<String>) {
        self.push(ToastKind::Success, message);
    }

    pub fn error(&self, message: impl Into<String>) {
        self.push(ToastKind::Error, message);
    }

    /// Removing by id rather than index, since the list shifts as older toasts
    /// expire underneath a newer one.
    pub fn dismiss(&self, id: u32) {
        self.items
            .update(|list| list.retain(|toast| toast.id != id));
    }
}

pub fn provide_toasts() -> Toasts {
    let toasts = Toasts {
        items: RwSignal::new(Vec::new()),
        next_id: RwSignal::new(0),
    };
    provide_context(toasts);
    toasts
}

pub fn use_toasts() -> Toasts {
    expect_context::<Toasts>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub() -> Toasts {
        Toasts {
            items: RwSignal::new(Vec::new()),
            next_id: RwSignal::new(0),
        }
    }

    #[test]
    fn capacity_follows_the_viewport_and_never_reaches_zero() {
        assert_eq!(capacity_for(800.0), 7);
        assert_eq!(capacity_for(1200.0), 12);
        // A short window still shows one rather than swallowing the message.
        assert_eq!(capacity_for(200.0), 1);
        assert_eq!(capacity_for(0.0), 1);
    }

    /// A burst past capacity drops from the top, so the newest is always the
    /// one on screen.
    #[test]
    fn a_full_stack_drops_the_oldest() {
        let toasts = hub();
        let capacity = capacity_for(ASSUMED_VIEWPORT_PX);
        for n in 0..capacity + 2 {
            toasts.push(ToastKind::Error, format!("problem {n}"));
        }
        let items = toasts.items.get_untracked();
        assert_eq!(items.len(), capacity);
        assert_eq!(items[0].message, "problem 2", "the two oldest were dropped");
        assert_eq!(
            items[capacity - 1].message,
            format!("problem {}", capacity + 1)
        );
    }

    #[test]
    fn dismissing_removes_only_that_toast() {
        let toasts = hub();
        toasts.success("first");
        toasts.error("second");
        let id = toasts.items.get_untracked()[0].id;
        toasts.dismiss(id);
        let left = toasts.items.get_untracked();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].message, "second");
    }

    #[test]
    fn an_error_lingers_far_longer_than_a_success() {
        assert_eq!(ToastKind::Success.lifetime_ms(), 3_000);
        assert_eq!(ToastKind::Warning.lifetime_ms(), 3_000);
        assert_eq!(ToastKind::Error.lifetime_ms(), 10_000);
    }
}
