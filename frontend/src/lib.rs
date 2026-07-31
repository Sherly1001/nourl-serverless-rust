pub mod app;
pub mod theme;

use leptos::prelude::*;

pub fn run() {
    console_error_panic_hook::set_once();
    mount_to_body(app::App);
}
