use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::UserInfo;

use crate::api;

/// Current session, shared through context so everything agrees without
/// prop-drilling.
#[derive(Clone, Copy)]
pub struct AuthContext {
    /// `None` both in flight and logged out — watch `loaded` to tell them
    /// apart, or a page flashes its signed-out state first.
    pub user: RwSignal<Option<UserInfo>>,
    pub loaded: RwSignal<bool>,
}

impl AuthContext {
    pub fn is_admin(&self) -> bool {
        self.user.get().is_some_and(|u| u.is_admin)
    }

    /// The settings are the root's alone, so the nav entry asks separately.
    pub fn is_root(&self) -> bool {
        self.user.get().is_some_and(|u| u.is_root)
    }

    /// An error reads as logged out on purpose: 401 is the normal answer for
    /// an anonymous visitor.
    pub fn refresh(&self) {
        let user = self.user;
        let loaded = self.loaded;
        spawn_local(async move {
            user.set(api::me().await.ok());
            loaded.set(true);
        });
    }
}

/// Installs the context and kicks off the first load.
pub fn provide_auth() -> AuthContext {
    let ctx = AuthContext {
        user: RwSignal::new(None),
        loaded: RwSignal::new(false),
    };
    provide_context(ctx);
    ctx.refresh();
    ctx
}

pub fn use_auth() -> AuthContext {
    expect_context::<AuthContext>()
}
