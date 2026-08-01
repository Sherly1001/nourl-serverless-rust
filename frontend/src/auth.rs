use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::UserInfo;

use crate::api;

/// Current session, shared through context so the navbar, the My URLs page and
/// the login form all agree without prop-drilling.
#[derive(Clone, Copy)]
pub struct AuthContext {
    /// `None` both while the first `/api/auth/me` is in flight and when logged
    /// out — watch `loaded` to tell those apart, or a page will flash its
    /// signed-out state before the session resolves.
    pub user: RwSignal<Option<UserInfo>>,
    pub loaded: RwSignal<bool>,
}

impl AuthContext {
    pub fn is_admin(&self) -> bool {
        self.user.get().is_some_and(|u| u.is_admin)
    }

    /// Re-reads `/api/auth/me`. Called once at startup and after login/logout.
    /// An error is indistinguishable from logged out on purpose: a 401 is the
    /// normal answer for an anonymous visitor.
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
