use crate::{LightcraftApp, Services, headless::Headless};
use serde_json::json;
use std::{
    path::Path,
    time::{Duration, Instant},
};

fn headless(size: [f32; 2]) -> Headless {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default());
    app.ui.settings.gpu = false;
    app.ui.language = crate::i18n::Locale::En;
    let mut h = Headless::new(app, size, 1.0);
    h.step();
    h.step();
    h
}
fn wait(h: &mut Headless) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while h.app.cloud.busy && Instant::now() < deadline {
        h.step();
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(!h.app.cloud.busy, "Cloud worker did not finish");
    h.step();
    h.step();
}
fn good(h: &mut Headless) {
    wait(h);
    assert!(h.app.cloud.error.is_none(), "{:?}", h.app.cloud.error);
}
fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id":id}), Duration::from_secs(10));
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}
fn screenshot(h: &mut Headless, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let image = h.snapshot(Duration::from_secs(30));
    let image = lightcraft_raster::Rgba8 { width: image.size[0], height: image.size[1], data: image.pixels.iter().map(|c| c.to_array()).collect() };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&image), &Default::default())?;
    std::fs::write(path, bytes)?;
    Ok(())
}
#[test]
fn cloud_settings_and_empty_workspace_fit_small_and_large_windows() {
    for size in [[680.0, 640.0], [1440.0, 960.0]] {
        let mut h = headless(size);
        click(&mut h, "icon:cloud");
        assert!(h.app.cloud.open);
        assert!(h.app.widgets.iter().any(|(id, _)| id == "button:cloud.settings"));
        click(&mut h, "button:cloud.settings");
        assert!(matches!(&h.app.ui.dialog,Some(crate::state::Dialog::Settings{tab}) if tab=="cloud"));
        let server = h.app.widgets.iter().find(|(id, _)| id == "field:cloudServer").unwrap().1;
        assert!(server.left() >= 0.0 && server.right() <= size[0] + 1.0, "{server:?}");
        assert!(!super::enabled(&h.app, "cloud.login"));
        assert!(!super::enabled(&h.app, "cloud.upload"));
    }
}
#[test]
fn cloud_settings_persist_only_public_configuration_and_local_edits_are_preserved() {
    let mut h = headless([1100.0, 800.0]);
    let active = h.app.session.active().unwrap();
    h.app.run("photo.rate", json!({"rating":4})).unwrap();
    let before = h.app.session.catalog.photo(active).unwrap().rating;
    h.app.ui.settings.cloud_server = "https://photos.example.com".into();
    let saved = serde_json::to_string(&h.app.ui).unwrap();
    let restored: crate::UiState = serde_json::from_str(&saved).unwrap();
    assert_eq!(restored.settings.cloud_server, "https://photos.example.com");
    assert!(!saved.contains("access_token") && !saved.contains("refresh_token") && !saved.contains("password"));
    h.app.ui.settings.cloud_server.clear();
    h.app.run("cloud.open", json!({})).unwrap();
    assert!(h.app.run("photo.rate", json!({"rating":1})).is_err());
    let _ = h.request("ui.key", json!({"key":"1"}), Duration::from_secs(5));
    h.step();
    assert_eq!(h.app.session.catalog.photo(active).unwrap().rating, before);
    h.app.run("cloud.local", json!({})).unwrap();
    h.app.run("photo.rate", json!({"rating":2})).unwrap();
    assert_eq!(h.app.session.catalog.photo(active).unwrap().rating, 2);
}
#[test]
fn closing_during_a_cloud_operation_requires_an_explicit_choice() {
    let mut h = headless([1000.0, 760.0]);
    h.app.cloud.busy = true;
    assert!(!crate::panels::notices::may_close(&mut h.app));
    assert!(matches!(h.app.quit_prompt, Some(crate::QuitPrompt::Tasks(_))));
    h.step();
    h.step();
    click(&mut h, "button:quitCancel");
    assert!(h.app.cloud.busy);
    assert!(!h.app.ui.quit);
    assert!(!crate::panels::notices::may_close(&mut h.app));
    h.step();
    h.step();
    click(&mut h, "button:quitAnyway");
    assert!(!h.app.cloud.busy);
    assert!(h.quit_requested());
    assert!(h.app.quit_prompt.is_none());
    assert!(crate::panels::notices::may_close(&mut h.app));
}

#[test]
fn invalid_cloud_address_is_rejected_before_network_or_settings_change() {
    let mut h = headless([1000.0, 760.0]);
    for server in ["not a URL", "http://example.com", "https://user:secret@example.com", "https://example.com/?token=secret"] {
        assert!(h.app.run("cloud.configure", json!({"server":server})).is_err());
        assert!(!h.app.cloud.busy);
        assert!(h.app.ui.settings.cloud_server.is_empty());
    }
}

#[test]
fn cloud_real_backend_browser_upload_download_restart_and_logout() {
    let Ok(server) = std::env::var("LC_CLOUD_UI_TEST_SERVER") else {
        lightcraft_testkit::skip!(System, "local Cloud/Keycloak and isolated OS credential service not configured");
        return;
    };
    assert!(server.starts_with("http://127.0.0.1:"));
    let output = std::path::PathBuf::from(std::env::var("LC_CLOUD_UI_TEST_OUTPUT").unwrap());
    std::fs::create_dir_all(&output).unwrap();
    if std::env::var("LC_CLOUD_UI_TEST_RESTART").is_ok() {
        let ui: crate::UiState = serde_json::from_slice(&std::fs::read(output.join("ui.json")).unwrap()).unwrap();
        let mut h = headless([1440.0, 960.0]);
        h.app.ui = ui;
        h.app.run("cloud.open", json!({})).unwrap();
        good(&mut h);
        assert!(h.app.cloud.identity.is_some());
        assert!(h.app.cloud.remembered);
        assert_eq!(h.app.cloud.photos.len(), 5);
        let photo = h.app.cloud.photos.iter().find(|p| p.original_name == "Alpine_Light.jpg").unwrap().clone();
        h.app.run("cloud.selectPhoto", json!({"id":photo.id})).unwrap();
        h.app.run("cloud.preview", json!({})).unwrap();
        good(&mut h);
        assert!(h.app.cloud.thumbnails.contains_key(&photo.id));
        screenshot(&mut h, &output.join("cloud-restored.png")).unwrap();
        h.app.run("cloud.settings", json!({})).unwrap();
        h.step();
        h.step();
        click(&mut h, "button:cloud.logout");
        good(&mut h);
        assert!(h.app.cloud.identity.is_none());
        assert!(h.app.cloud.photos.is_empty());
        screenshot(&mut h, &output.join("cloud-signed-out.png")).unwrap();
        return;
    }
    let helper = std::env::var("LC_CLOUD_UI_BROWSER_HELPER").unwrap();
    let mut h = headless([1440.0, 960.0]);
    h.app.services.open_url = Some(Box::new(move |url| {
        use std::io::Write;
        let mut child = std::process::Command::new("python3")
            .arg(&helper)
            .arg("login")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut stdin = child.stdin.take().ok_or("Browser input unavailable")?;
        writeln!(stdin, "{url}").map_err(|e| e.to_string())?;
        drop(stdin);
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }));
    click(&mut h, "icon:cloud");
    click(&mut h, "button:cloud.settings");
    h.app.cloud.server_draft = server.clone();
    click(&mut h, "button:cloud.check");
    good(&mut h);
    assert!(h.app.cloud.connected);
    screenshot(&mut h, &output.join("cloud-settings.png")).unwrap();
    click(&mut h, "button:cloud.login");
    good(&mut h);
    assert!(h.app.cloud.identity.is_some());
    assert!(h.app.cloud.remembered, "OS credential store must work in this integration test");
    let identity = h.app.cloud.identity.as_ref().unwrap().id;
    click(&mut h, "button:cloud.check");
    good(&mut h);
    assert_eq!(h.app.cloud.identity.as_ref().unwrap().id, identity);
    click(&mut h, "button:cloud.open");
    assert!(h.app.ui.dialog.is_none(), "Open Cloud Library must close Settings");
    h.app.cloud.new_library = "Codex Cloud UI · test library".into();
    click(&mut h, "button:cloud.createLibrary");
    good(&mut h);
    assert!(h.app.cloud.library.is_some(), "Create library must run through its visible button");
    assert!(h.app.cloud.photos.is_empty());
    screenshot(&mut h, &output.join("cloud-empty.png")).unwrap();
    let source = tempfile::tempdir().unwrap();
    let ids: Vec<_> = h.app.session.catalog.photos().take(4).map(|p| p.id).collect();
    let names = ["Alpine_Light.jpg", "Forest_Morning.jpg", "Coastal_Evening.jpg", "City_Details.jpg"];
    let mut paths = Vec::new();
    let mut first = Vec::new();
    for (id, name) in ids.into_iter().zip(names) {
        let options = lightcraft_engine::export::ExportOptions {
            resize: Some(lightcraft_engine::export::Resize { value: 960.0, ..Default::default() }),
            ..Default::default()
        };
        let exported = lightcraft_engine::export::export_photo(&mut h.app.session, id, &options, 0).unwrap();
        if first.is_empty() {
            first = exported.bytes.clone();
        }
        let path = source.path().join(name);
        std::fs::write(&path, exported.bytes).unwrap();
        paths.push(path);
    }
    // A procedural TIFF/DNG envelope with an embedded JPEG, independent of any camera corpus.
    let mut raw = b"II\x2a\0\x08\0\0\0".to_vec();
    raw.extend(3u16.to_le_bytes());
    let offset = 8 + 2 + 3 * 12 + 4;
    for (tag, kind, count, value) in [(513u16, 4u16, 1u32, offset as u32), (514, 4, 1, first.len() as u32), (50706, 1, 4, 0x00000401)] {
        raw.extend(tag.to_le_bytes());
        raw.extend(kind.to_le_bytes());
        raw.extend(count.to_le_bytes());
        raw.extend(value.to_le_bytes());
    }
    raw.extend(0u32.to_le_bytes());
    raw.extend(&first);
    let raw_path = source.path().join("Mountain_RAW.dng");
    std::fs::write(&raw_path, &raw).unwrap();
    paths.push(raw_path);
    h.app.services.pick_files = Some(Box::new(move || paths.iter().map(|p| p.to_string_lossy().to_string()).collect()));
    click(&mut h, "button:cloud.upload");
    good(&mut h);
    assert!(h.app.cloud.progress.as_ref().is_some_and(|p| p.done == p.total && p.total > 0));
    assert_eq!(h.app.cloud.photos.len(), 5);
    assert!(h.app.cloud.thumbnails.len() >= 4);
    let photo = h.app.cloud.photos.iter().find(|p| p.original_name == "Mountain_RAW.dng").unwrap().clone();
    h.app.run("cloud.selectPhoto", json!({"id":photo.id})).unwrap();
    h.step();
    h.step();
    screenshot(&mut h, &output.join("cloud-library.png")).unwrap();
    let downloaded = output.join("downloaded-original.dng");
    let _ = std::fs::remove_file(&downloaded);
    h.app.run("cloud.download", json!({"path":downloaded})).unwrap();
    good(&mut h);
    assert_eq!(std::fs::read(&downloaded).unwrap(), raw);
    // Existing local files are protected even after a save dialog accepts their name.
    h.app.run("cloud.download", json!({"path":downloaded})).unwrap();
    wait(&mut h);
    assert!(h.app.cloud.error.as_ref().unwrap().contains("already exists"));
    assert_eq!(std::fs::read(&downloaded).unwrap(), raw);
    h.app.cloud.error = None;
    h.app.run("cloud.settings", json!({})).unwrap();
    h.step();
    h.step();
    screenshot(&mut h, &output.join("cloud-account.png")).unwrap();
    let saved = serde_json::to_vec_pretty(&h.app.ui).unwrap();
    assert!(!String::from_utf8_lossy(&saved).contains("refresh_token"));
    std::fs::write(output.join("ui.json"), saved).unwrap();
    drop(h);
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "cloud::tests::cloud_real_backend_browser_upload_download_restart_and_logout", "--nocapture"])
        .env("LC_CLOUD_UI_TEST_RESTART", "1")
        .output()
        .unwrap();
    assert!(result.status.success(), "restart process: {}\n{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
    let mut h = headless([760.0, 720.0]);
    h.app.ui = serde_json::from_slice(&std::fs::read(output.join("ui.json")).unwrap()).unwrap();
    h.app.run("cloud.open", json!({})).unwrap();
    good(&mut h);
    assert!(h.app.cloud.identity.is_none(), "logout removed the OS-stored refresh token");
    screenshot(&mut h, &output.join("cloud-compact.png")).unwrap();
    h.app.run("cloud.configure", json!({"server":"http://127.0.0.1:9"})).unwrap();
    wait(&mut h);
    assert!(h.app.cloud.error.is_some());
    screenshot(&mut h, &output.join("cloud-network-error.png")).unwrap();
}
