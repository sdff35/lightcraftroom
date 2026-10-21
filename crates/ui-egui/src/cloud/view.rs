use crate::{
    LightcraftApp,
    icons::{Icon, paint},
    theme::Tokens,
    widgets::register,
};
use egui::{RichText, vec2};
use serde_json::json;

fn action(app: &mut LightcraftApp, ui: &mut egui::Ui, id: &str, label: &str, primary: bool) {
    let t = Tokens::get(ui.ctx());
    let mut button =
        egui::Button::new(RichText::new(crate::i18n::tr(label)).color(if primary { egui::Color32::WHITE } else { t.text })).min_size(vec2(0.0, 29.0));
    if primary {
        button = button.fill(t.accent);
    }
    let response = ui.add_enabled(super::enabled(app, id), button);
    register(ui.ctx(), format!("button:{id}"), response.rect);
    if response.clicked()
        && let Err(error) = app.run(id, json!({}))
    {
        app.cloud.error = Some(error);
    }
}
pub(super) fn bytes(n: u64) -> String {
    if n >= 1048576 { format!("{:.1} MB", n as f64 / 1048576.0) } else { format!("{:.1} KB", n as f64 / 1024.0) }
}
fn notice(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    if let Some(error) = app.cloud.error.clone() {
        egui::Frame::NONE.fill(t.inset).inner_margin(12.0).corner_radius(5).show(ui, |ui| {
            ui.label(RichText::new(error).color(t.caution));
            if super::enabled(app, "cloud.retry") {
                action(app, ui, "cloud.retry", "Retry transfer", false);
            }
        });
    }
    if app.cloud.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(&app.cloud.status);
            action(app, ui, "cloud.cancel", "Cancel", false);
        });
        if let Some(p) = &app.cloud.progress {
            let fraction = if p.total > 0 { (p.done as f64 / p.total as f64).clamp(0.0, 1.0) as f32 } else { 0.0 };
            ui.add(egui::ProgressBar::new(fraction).text(format!("{} · {} / {}", p.name, bytes(p.done), bytes(p.total))));
        }
        if app.cloud.browser.is_some() {
            action(app, ui, "cloud.browser", "Open sign-in page", false);
        }
    }
}
pub fn settings(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(8.0);
    ui.label(RichText::new(crate::i18n::tr("Your LightCraft Cloud")).font(t.semibold(19.0)));
    ui.label(RichText::new(crate::i18n::tr("Connect to your own server and keep your originals under your control.")).color(t.text_dim));
    ui.add_space(16.0);
    ui.label(crate::i18n::tr("Server address"));
    ui.add_enabled_ui(!app.cloud.busy, |ui| {
        crate::text_field::TextField::singleline("field:cloudServer", &mut app.cloud.server_draft)
            .hint("https://cloud.example.com")
            .width(ui.available_width())
            .show(ui);
    });
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        action(app, ui, "cloud.check", "Save & test connection", false);
        ui.label(RichText::new(&app.cloud.status).color(if app.cloud.connected { t.pick } else { t.text_dim }));
    });
    ui.add_space(16.0);
    ui.separator();
    ui.add_space(12.0);
    ui.label(RichText::new(crate::i18n::tr("Account")).font(t.semibold(13.0)));
    if let Some(identity) = &app.cloud.identity {
        ui.label(crate::i18n::tr("Signed in to your Cloud account"));
        ui.label(RichText::new(format!("{}…", identity.subject.chars().take(16).collect::<String>())).small().color(t.text_dim));
        ui.label(
            RichText::new(crate::i18n::tr(if app.cloud.remembered {
                "Sign-in saved in the system credential store"
            } else {
                "Sign-in kept for this session only"
            }))
            .small()
            .color(t.text_dim),
        );
        ui.horizontal_wrapped(|ui| {
            action(app, ui, "cloud.open", "Open Cloud Library", true);
            action(app, ui, "cloud.logout", "Sign out", false);
        });
    } else {
        ui.add_enabled_ui(!app.cloud.busy, |ui| {
            let r = ui.checkbox(&mut app.ui.settings.cloud_remember, crate::i18n::tr("Remember sign-in on this device"));
            register(ui.ctx(), "check:cloudRemember", r.rect);
        });
        ui.label(
            RichText::new(crate::i18n::tr("Sign in securely in your system browser. LightCraft does not ask for your password."))
                .small()
                .color(t.text_dim),
        );
        action(app, ui, "cloud.login", "Sign in with browser", true);
    }
    if app.cloud.vault_warning {
        ui.add_space(8.0);
        ui.label(
            RichText::new(crate::i18n::tr(
                "The system credential store is unavailable. Sign-in can be kept in memory for this session; unlock the store to remember it.",
            ))
            .small()
            .color(t.caution),
        );
    }
    ui.add_space(12.0);
    notice(app, ui);
}
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::top("cloud_workspace_header").frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(20, 12))).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            action(app, ui, "cloud.local", "← Local Library", false);
            ui.separator();
            ui.label(RichText::new("Cloud Library").font(t.semibold(21.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                action(app, ui, "cloud.settings", "Cloud settings", false);
                if app.cloud.identity.is_some() {
                    action(app, ui, "cloud.refresh", "Refresh", false);
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let colour = if app.cloud.identity.is_some() && app.cloud.connected { t.pick } else { t.text_dim };
            let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 3.0, colour);
            ui.label(
                RichText::new(if app.ui.settings.cloud_server.is_empty() { "Your independent photo library" } else { &app.ui.settings.cloud_server })
                    .small()
                    .color(t.text_dim),
            );
            if !app.cloud.busy {
                ui.label(RichText::new(&app.cloud.status).small().color(t.text_dim));
            }
        });
    });
    let wide = ui.available_width() >= 820.0;
    if wide {
        egui::Panel::left("cloud_libraries")
            .exact_size(224.0)
            .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(16.0))
            .show(ui, |ui| sidebar(app, ui));
    }
    let detail = ui.available_width() >= 1000.0 && app.cloud.selected.is_some();
    if detail {
        egui::Panel::right("cloud_inspector")
            .exact_size(248.0)
            .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(16.0))
            .show(ui, |ui| inspector(app, ui));
    }
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.grid_bg).inner_margin(20.0)).show(ui, |ui| {
        if !wide {
            egui::CollapsingHeader::new(crate::i18n::tr("Cloud libraries")).default_open(app.cloud.library.is_none()).show(ui, |ui| sidebar(app, ui));
            ui.separator();
        }
        notice(app, ui);
        if app.cloud.identity.is_none() {
            ui.add_space(48.0);
            empty(ui, &t, "Your photos. Your cloud.", "Store original RAW and JPEG files on your own LightCraft server.");
            ui.add_space(14.0);
            ui.horizontal_wrapped(|ui| {
                if app.cloud.connected {
                    action(app, ui, "cloud.login", "Sign in with browser", true);
                } else {
                    action(app, ui, "cloud.settings", "Connect your server", true);
                }
            });
            return;
        }
        let name =
            app.cloud.libraries.iter().find(|l| Some(l.id) == app.cloud.library).map(|l| l.name.as_str()).unwrap_or("Cloud Library").to_owned();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(name).font(t.semibold(20.0)));
            ui.label(
                RichText::new(format!("{} photos loaded", app.cloud.photos.iter().filter(|p| p.status != "cancelled").count()))
                    .small()
                    .color(t.text_dim),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| action(app, ui, "cloud.upload", "Upload RAW / JPEG", true));
        });
        if !detail && app.cloud.selected.is_some() {
            ui.horizontal_wrapped(|ui| {
                action(app, ui, "cloud.download", "Download original", false);
                action(app, ui, "cloud.preview", "Load image", false);
            });
        }
        ui.add_space(12.0);
        if app.cloud.library.is_none() {
            empty(ui, &t, "Create your first Cloud library", "Give your library a name in the sidebar to start uploading originals.");
            return;
        }
        if app.cloud.photos.is_empty() && !app.cloud.busy {
            empty(ui, &t, "No photos yet", "Upload RAW or JPEG files. Your originals are stored without changes.");
            return;
        }
        ui.label(
            RichText::new(crate::i18n::tr("Images load from original files. Server previews and albums are not available yet."))
                .small()
                .color(t.text_dim),
        );
        ui.add_space(10.0);
        let filter = app.cloud.filter.to_lowercase();
        let photos: Vec<_> =
            app.cloud.photos.iter().filter(|p| p.status != "cancelled" && p.original_name.to_lowercase().contains(&filter)).cloned().collect();
        let columns = ((ui.available_width() + 12.0) / 200.0).floor().max(1.0) as usize;
        let cell_w = ((ui.available_width() - 12.0 * (columns.saturating_sub(1)) as f32) / columns as f32).max(120.0);
        egui::ScrollArea::vertical().id_salt("cloud_grid_scroll").show_rows(ui, 222.0, photos.len().div_ceil(columns), |ui, rows| {
            for row in rows {
                ui.horizontal(|ui| {
                    for photo in photos.iter().skip(row.saturating_mul(columns)).take(columns) {
                        let selected = app.cloud.selected == Some(photo.id);
                        let (rect, response) = ui.allocate_exact_size(vec2(cell_w, 210.0), egui::Sense::click());
                        register(ui.ctx(), format!("cloudPhoto:{}", photo.id), rect);
                        crate::widgets::announce(&response, &photo.original_name);
                        if response.clicked() {
                            let _ = app.run("cloud.selectPhoto", json!({"id":photo.id}));
                        }
                        let bg = if selected {
                            t.cell_selected
                        } else if response.hovered() {
                            t.hover
                        } else {
                            t.cell
                        };
                        ui.painter().rect_filled(rect, 5, bg);
                        if selected {
                            ui.painter().rect_stroke(rect, 5, egui::Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
                        }
                        let image_rect = egui::Rect::from_min_max(rect.min + vec2(9.0, 9.0), rect.min + vec2(cell_w - 9.0, 151.0));
                        ui.painter().rect_filled(image_rect, 3, t.canvas);
                        if let Some(image) = app.cloud.thumbnails.get(&photo.id) {
                            let size = image.size_vec2();
                            let ratio = (image_rect.width() / size.x).min(image_rect.height() / size.y);
                            let dest = egui::Rect::from_center_size(image_rect.center(), size * ratio);
                            ui.painter().image(
                                image.id(),
                                dest,
                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        } else {
                            paint(
                                ui.painter(),
                                egui::Rect::from_center_size(image_rect.center() - vec2(0.0, 12.0), vec2(34.0, 34.0)),
                                Icon::Cloud,
                                t.text_dim,
                            );
                            ui.painter().text(
                                image_rect.center() + vec2(0.0, 24.0),
                                egui::Align2::CENTER_CENTER,
                                if photo.media_type == "image/jpeg" { "JPEG original" } else { "RAW original" },
                                t.semibold(11.0),
                                t.text_dim,
                            );
                        }
                        let filename = ui.painter().layout(photo.original_name.clone(), t.semibold(12.0), t.text, cell_w - 20.0);
                        let clip = egui::Rect::from_min_size(rect.min + vec2(10.0, 158.0), vec2(cell_w - 20.0, 18.0));
                        ui.painter().with_clip_rect(clip).galley(clip.min, filename, t.text);
                        let format = photo.original_name.rsplit('.').next().unwrap_or("RAW").to_uppercase();
                        ui.painter().text(
                            rect.min + vec2(10.0, 181.0),
                            egui::Align2::LEFT_TOP,
                            format!("{format}  ·  {}", bytes(photo.byte_length)),
                            t.font(10.5),
                            t.text_dim,
                        );
                        ui.painter().text(
                            rect.right_bottom() - vec2(10.0, 10.0),
                            egui::Align2::RIGHT_BOTTOM,
                            if photo.status == "ready" { "Stored" } else { "Pending" },
                            t.font(10.5),
                            if photo.status == "ready" { t.pick } else { t.caution },
                        );
                    }
                });
            }
        });
        if app.cloud.next.is_some() {
            action(app, ui, "cloud.more", "Load more photos", false);
        }
    });
}
fn empty(ui: &mut egui::Ui, t: &Tokens, title: &str, text: &str) {
    ui.add_space(20.0);
    ui.label(RichText::new(crate::i18n::tr(title)).font(t.semibold(24.0)).color(t.text));
    ui.add_space(8.0);
    ui.label(RichText::new(crate::i18n::tr(text)).color(t.text_dim));
}
fn sidebar(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.label(RichText::new(crate::i18n::tr("LIBRARIES")).font(t.semibold(11.0)).color(t.text_dim));
    ui.add_space(12.0);
    let libraries = app.cloud.libraries.clone();
    for library in libraries {
        let response = ui.add_enabled(
            !app.cloud.busy,
            egui::Button::new(&library.name).selected(app.cloud.library == Some(library.id)).min_size(vec2(ui.available_width(), 32.0)),
        );
        register(ui.ctx(), format!("cloudLibrary:{}", library.id), response.rect);
        if response.clicked()
            && let Err(e) = app.run("cloud.selectLibrary", json!({"id":library.id}))
        {
            app.cloud.error = Some(e);
        }
    }
    if app.cloud.identity.is_some() {
        ui.add_space(16.0);
        crate::text_field::TextField::singleline("field:cloudLibraryName", &mut app.cloud.new_library)
            .hint(crate::i18n::tr("New library name"))
            .width(ui.available_width())
            .show(ui);
        action(app, ui, "cloud.createLibrary", "Create library", false);
    }
    ui.add_space(28.0);
    ui.separator();
    ui.add_space(12.0);
    ui.label(RichText::new(crate::i18n::tr("ALBUMS")).font(t.semibold(11.0)).color(t.text_dim));
    ui.label(RichText::new(crate::i18n::tr("Albums are not supported by this server version.")).small().color(t.text_dim));
    ui.add_space(24.0);
    ui.label(RichText::new(crate::i18n::tr("Originals stay unchanged")).font(t.semibold(12.0)));
    ui.label(RichText::new(crate::i18n::tr("RAW and JPEG files are verified with SHA-256 when transferred.")).small().color(t.text_dim));
}
fn inspector(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let Some(photo) = app.cloud.photos.iter().find(|p| Some(p.id) == app.cloud.selected).cloned() else {
        return;
    };
    ui.label(RichText::new(crate::i18n::tr("ORIGINAL")).font(t.semibold(11.0)).color(t.text_dim));
    ui.add_space(16.0);
    ui.label(RichText::new(&photo.original_name).font(t.semibold(16.0)));
    ui.add_space(12.0);
    for (key, value) in [
        ("Format", photo.original_name.rsplit('.').next().unwrap_or("RAW").to_uppercase()),
        ("File size", bytes(photo.byte_length)),
        ("Storage", photo.status.clone()),
        ("Revision", photo.revision.to_string()),
    ] {
        ui.label(RichText::new(crate::i18n::tr(key)).small().color(t.text_dim));
        ui.label(value);
        ui.add_space(8.0);
    }
    ui.label(RichText::new("SHA-256").small().color(t.text_dim));
    ui.label(RichText::new(photo.sha256).monospace().small());
    ui.add_space(18.0);
    action(app, ui, "cloud.download", "Download original", true);
    ui.add_space(6.0);
    action(app, ui, "cloud.preview", "Load image", false);
    ui.add_space(8.0);
    ui.label(
        RichText::new(crate::i18n::tr("Loading an image downloads the original temporarily. RAW files need an embedded preview."))
            .small()
            .color(t.text_dim),
    );
    if app.cloud.preview_errors.contains(&photo.id) {
        ui.label(RichText::new(crate::i18n::tr("No supported preview in this original.")).small().color(t.caution));
    }
}
