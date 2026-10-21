//! Independent Cloud workspace, driven through the same commands as the local editor.
#[cfg(not(target_arch = "wasm32"))]
mod desktop;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
#[cfg(not(target_arch = "wasm32"))]
mod view;
#[cfg(not(target_arch = "wasm32"))]
pub use desktop::{State, command, enabled, pump};
#[cfg(not(target_arch = "wasm32"))]
pub use view::{settings, show};

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
pub struct State {
    pub open: bool,
    pub filter: String,
}
#[cfg(target_arch = "wasm32")]
pub fn command(_: &mut crate::LightcraftApp, _: &str, _: &serde_json::Value) -> Result<serde_json::Value, String> {
    Err("Cloud is available in the desktop application.".into())
}
#[cfg(target_arch = "wasm32")]
pub fn enabled(_: &crate::LightcraftApp, _: &str) -> bool {
    false
}
#[cfg(target_arch = "wasm32")]
pub fn pump(_: &mut crate::LightcraftApp, _: &egui::Context) {}
#[cfg(target_arch = "wasm32")]
pub fn settings(_: &mut crate::LightcraftApp, ui: &mut egui::Ui) {
    ui.label("Cloud is available in the desktop application.");
}
#[cfg(target_arch = "wasm32")]
pub fn show(app: &mut crate::LightcraftApp, ui: &mut egui::Ui) {
    settings(app, ui);
}
