use crate::{
    LightcraftApp,
    pick::{self, PickRequest, Picked},
};
use lightcraft_cloud_client::{
    Client, Error, Identity, Library, Page, Photo, Progress, Uuid,
    vault::{SystemVault, TokenVault},
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

pub(super) const EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "dng", "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "rwl", "pef", "srw", "3fr", "fff", "iiq", "mef",
    "mos", "erf", "kdc", "dcr", "raw",
];
type Result<T> = std::result::Result<T, Error>;
pub struct State {
    pub open: bool,
    pub server_draft: String,
    pub initialized: bool,
    pub connected: bool,
    pub identity: Option<Identity>,
    pub remembered: bool,
    pub vault_warning: bool,
    pub busy: bool,
    cancellable: bool,
    pub status: String,
    pub error: Option<String>,
    pub libraries: Vec<Library>,
    pub photos: Vec<Photo>,
    pub library: Option<Uuid>,
    pub selected: Option<Uuid>,
    pub next: Option<Uuid>,
    pub filter: String,
    pub new_library: String,
    pub progress: Option<Progress>,
    pub browser: Option<String>,
    pub thumbnails: HashMap<Uuid, egui::TextureHandle>,
    pub preview_errors: HashSet<Uuid>,
    pub last_download: Option<PathBuf>,
    pub vault: Arc<dyn TokenVault>,
    client: Option<Arc<Mutex<Client>>>,
    receiver: Option<mpsc::Receiver<Event>>,
    progress_slot: Arc<Mutex<Option<Progress>>>,
    cancel: Arc<AtomicBool>,
    retry: Option<Job>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            server_draft: String::new(),
            initialized: false,
            connected: false,
            identity: None,
            remembered: false,
            vault_warning: false,
            busy: false,
            cancellable: false,
            status: "Connect your own Cloud server".into(),
            error: None,
            libraries: Vec::new(),
            photos: Vec::new(),
            library: None,
            selected: None,
            next: None,
            filter: String::new(),
            new_library: String::new(),
            progress: None,
            browser: None,
            thumbnails: HashMap::new(),
            preview_errors: HashSet::new(),
            last_download: None,
            vault: Arc::new(SystemVault),
            client: None,
            receiver: None,
            progress_slot: Default::default(),
            cancel: Default::default(),
            retry: None,
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
#[derive(Clone)]
enum Job {
    Connect(String),
    Login(bool),
    Logout,
    Libraries,
    Photos(Uuid, Option<Uuid>),
    Create(String),
    Upload(Uuid, Vec<(PathBuf, Uuid)>),
    Download(Photo, PathBuf),
    Preview(Photo),
}
enum Outcome {
    Connected(Arc<Mutex<Client>>, Option<Identity>, bool, bool),
    LoggedIn(Identity, bool, bool),
    LoggedOut(Option<String>),
    Libraries(Vec<Library>),
    Photos(Uuid, Page<Photo>, bool),
    Created(Library),
    Uploaded,
    Downloaded(PathBuf),
    Preview(Uuid, Option<lightcraft_raster::Rgba8>),
}
enum Event {
    Browser(String),
    Image(Uuid, Option<lightcraft_raster::Rgba8>),
    Credentials(bool, bool),
    Done(Result<Outcome>),
}

fn thumbnail(path: &Path) -> Option<lightcraft_raster::Rgba8> {
    let file = std::fs::File::open(path).ok()?;
    // Thumbnail work reads bounded bytes, never invokes a sensor decode or allocates a full RAW.
    use std::io::Read;
    let length = file.metadata().ok()?.len();
    if length > 128 * 1024 * 1024 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(128 * 1024 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 128 * 1024 * 1024 {
        return None;
    }
    let data = if bytes.starts_with(&[0xff, 0xd8]) { bytes } else { lightcraft_raw::embedded_preview_covering(&bytes, 320)? };
    let thumb = lightcraft_codecs::decode_thumbnail_with(
        &data,
        &lightcraft_codecs::ThumbnailOptions { max_edge: 480, min_embedded_edge: 160, max_pixels: 32_000_000 },
    )
    .ok()?;
    Some(thumb.image.oriented(lightcraft_geom::Orientation::from_exif(thumb.orientation)))
}
impl State {
    pub(crate) fn cancel_for_exit(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.busy = false;
        self.receiver = None;
    }

    fn start(&mut self, job: Job) -> std::result::Result<(), String> {
        if self.busy {
            return Err("Wait for the current Cloud operation, or cancel it first.".into());
        }
        self.error = None;
        self.browser = None;
        if !matches!(job, Job::Libraries | Job::Photos(..)) {
            self.progress = None;
            if let Ok(mut p) = self.progress_slot.lock() {
                *p = None;
            }
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let progress = self.progress_slot.clone();
        let client = self.client.clone();
        let vault = self.vault.clone();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.busy = true;
        self.cancellable = matches!(job, Job::Login(_) | Job::Upload(..) | Job::Download(..) | Job::Preview(_));
        self.status = match &job {
            Job::Connect(_) => "Checking server…",
            Job::Login(_) => "Waiting for browser sign-in…",
            Job::Logout => "Signing out…",
            Job::Libraries | Job::Photos(..) => "Loading Cloud library…",
            Job::Create(_) => "Creating library…",
            Job::Upload(..) => "Preparing original…",
            Job::Download(..) => "Downloading original…",
            Job::Preview(_) => "Loading image from original…",
        }
        .into();
        if matches!(job, Job::Upload(..) | Job::Download(..)) {
            self.retry = Some(job.clone());
        }
        let spawned = std::thread::Builder::new().name("lightcraft-cloud".into()).spawn(move || {
            let credential_status = client.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Outcome> {
                if let Job::Connect(server) = job {
                    if let Some(existing) = &client {
                        let mut c = existing.lock().map_err(|_| Error::Network)?;
                        if c.server == server && c.identity.is_some() {
                            c.check_connection()?;
                            return Ok(Outcome::Connected(existing.clone(), c.identity.clone(), c.remembered, c.vault_warning));
                        }
                    }
                    let mut c = Client::connect(&server, vault)?;
                    match c.restore() {
                        Ok(_) | Err(Error::Unauthorized) => {}
                        Err(e) => return Err(e),
                    };
                    let (identity, remembered, warning) = (c.identity.clone(), c.remembered, c.vault_warning);
                    return Ok(Outcome::Connected(Arc::new(Mutex::new(c)), identity, remembered, warning));
                }
                let c = client.ok_or(Error::Network)?;
                let mut c = c.lock().map_err(|_| Error::Network)?;
                let notify: Arc<dyn Fn(Progress) + Send + Sync> = Arc::new(move |p| {
                    if let Ok(mut slot) = progress.lock() {
                        *slot = Some(p);
                    }
                });
                match job {
                    Job::Login(remember) => {
                        let login = c.begin_login()?;
                        let _ = tx.send(Event::Browser(login.url().to_owned()));
                        c.finish_login(login, &cancel, remember)?;
                        Ok(Outcome::LoggedIn(c.identity.clone().ok_or(Error::Unauthorized)?, c.remembered, c.vault_warning))
                    }
                    Job::Logout => {
                        let result = c.logout();
                        if c.identity.is_none() {
                            Ok(Outcome::LoggedOut(result.err().map(|e| e.to_string())))
                        } else {
                            result?;
                            Ok(Outcome::LoggedOut(None))
                        }
                    }
                    Job::Libraries => {
                        let mut libraries = Vec::new();
                        let mut after = None;
                        for _ in 0..2 {
                            let page = c.libraries(after)?;
                            libraries.extend(page.items);
                            after = page.next_after;
                            if after.is_none() {
                                return Ok(Outcome::Libraries(libraries));
                            }
                        }
                        Err(Error::Protocol)
                    }
                    Job::Photos(id, after) => Ok(Outcome::Photos(id, c.photos(id, after)?, after.is_some())),
                    Job::Create(name) => Ok(Outcome::Created(c.create_library(&name)?)),
                    Job::Upload(library, files) => {
                        for (path, operation) in files {
                            if cancel.load(Ordering::Relaxed) {
                                return Err(Error::Cancelled);
                            }
                            let p = c.upload(library, &path, operation, cancel.clone(), notify.clone())?;
                            let _ = tx.send(Event::Image(p.id, thumbnail(&path)));
                        }
                        Ok(Outcome::Uploaded)
                    }
                    Job::Download(photo, path) => {
                        c.download(&photo, &path, cancel, notify)?;
                        let _ = tx.send(Event::Image(photo.id, thumbnail(&path)));
                        Ok(Outcome::Downloaded(path))
                    }
                    Job::Preview(photo) => {
                        if photo.byte_length > 128 * 1024 * 1024 {
                            return Err(Error::File(
                                "This original is larger than the 128 MiB image-preview limit. Download it to view locally.".into(),
                            ));
                        }
                        let directory = tempfile::tempdir().map_err(|_| Error::Quota)?;
                        let path = directory.path().join("original");
                        c.download(&photo, &path, cancel, notify)?;
                        Ok(Outcome::Preview(photo.id, thumbnail(&path)))
                    }
                    Job::Connect(_) => Err(Error::Protocol),
                }
            }))
            .unwrap_or(Err(Error::Protocol));
            if let Some(client) = credential_status
                && let Ok(client) = client.lock()
            {
                let _ = tx.send(Event::Credentials(client.remembered, client.vault_warning));
            }
            let _ = tx.send(Event::Done(result));
        });
        if spawned.is_err() {
            self.busy = false;
            self.receiver = None;
            return Err("Could not start the Cloud worker.".into());
        }
        Ok(())
    }
    fn clear_account(&mut self) {
        self.identity = None;
        self.remembered = false;
        self.libraries.clear();
        self.photos.clear();
        self.thumbnails.clear();
        self.preview_errors.clear();
        self.library = None;
        self.selected = None;
        self.next = None;
        self.retry = None;
        self.last_download = None;
    }
    fn put_thumbnail(&mut self, ctx: &egui::Context, id: Uuid, image: Option<lightcraft_raster::Rgba8>) {
        if let Some(image) = image {
            if self.thumbnails.len() >= 48 {
                self.thumbnails.clear();
            }
            let color = egui::ColorImage::from_rgba_unmultiplied([image.width, image.height], image.data.as_flattened());
            self.thumbnails.insert(id, ctx.load_texture(format!("cloud-{id}"), color, egui::TextureOptions::LINEAR));
        } else {
            self.preview_errors.insert(id);
        }
    }
}
pub fn pump(app: &mut LightcraftApp, ctx: &egui::Context) {
    if !app.cloud.initialized {
        app.cloud.server_draft = app.ui.settings.cloud_server.clone();
        app.cloud.initialized = true;
    }
    let events: Vec<_> = app.cloud.receiver.as_ref().map(|r| r.try_iter().collect()).unwrap_or_default();
    let mut next = None;
    for event in events {
        match event {
            Event::Browser(url) => {
                app.cloud.browser = Some(url.clone());
                if app.services.open_url.as_mut().ok_or(()).and_then(|open| open(&url).map_err(|_| ())).is_err() {
                    app.cloud.error = Some(Error::Browser.to_string());
                }
            }
            Event::Image(id, image) => app.cloud.put_thumbnail(ctx, id, image),
            Event::Credentials(remembered, warning) => {
                app.cloud.remembered = remembered;
                app.cloud.vault_warning = warning;
            }
            Event::Done(result) => {
                app.cloud.busy = false;
                app.cloud.receiver = None;
                app.cloud.browser = None;
                match result {
                    Err(e) => {
                        if matches!(e, Error::Network | Error::Timeout | Error::Server(_)) {
                            app.cloud.connected = false;
                        }
                        if e == Error::Unauthorized {
                            app.cloud.clear_account();
                        }
                        if e == Error::Cancelled {
                            app.cloud.retry = None;
                        }
                        app.cloud.status = "Cloud operation did not complete".into();
                        app.cloud.error = Some(e.to_string());
                    }
                    Ok(outcome) => {
                        app.cloud.connected = true;
                        app.cloud.error = None;
                        app.cloud.status = "Connected".into();
                        match outcome {
                            Outcome::Connected(client, identity, remembered, warning) => {
                                app.cloud.clear_account();
                                app.cloud.client = Some(client);
                                app.cloud.connected = true;
                                app.cloud.identity = identity;
                                app.cloud.remembered = remembered;
                                app.cloud.vault_warning = warning;
                                if app.cloud.identity.is_some() {
                                    next = Some(Job::Libraries);
                                } else {
                                    app.cloud.status = "Server available · Sign in to open your libraries".into();
                                }
                            }
                            Outcome::LoggedIn(identity, remembered, warning) => {
                                app.cloud.identity = Some(identity);
                                app.cloud.remembered = remembered;
                                app.cloud.vault_warning = warning;
                                next = Some(Job::Libraries);
                            }
                            Outcome::LoggedOut(warning) => {
                                app.cloud.clear_account();
                                app.cloud.status = "Signed out".into();
                                app.cloud.error = warning;
                            }
                            Outcome::Libraries(libraries) => {
                                app.cloud.libraries = libraries;
                                let id = app
                                    .cloud
                                    .library
                                    .filter(|id| app.cloud.libraries.iter().any(|l| l.id == *id))
                                    .or_else(|| app.cloud.libraries.first().map(|l| l.id));
                                app.cloud.library = id;
                                if let Some(id) = id {
                                    next = Some(Job::Photos(id, None));
                                }
                            }
                            Outcome::Photos(library, page, append) => {
                                app.cloud.library = Some(library);
                                app.cloud.next = page.next_after;
                                if append {
                                    app.cloud.photos.extend(page.items);
                                } else {
                                    app.cloud.photos = page.items;
                                }
                                app.cloud.status = "Library up to date".into();
                            }
                            Outcome::Created(library) => {
                                app.cloud.library = Some(library.id);
                                app.cloud.new_library.clear();
                                next = Some(Job::Libraries);
                            }
                            Outcome::Uploaded => {
                                app.cloud.retry = None;
                                app.cloud.status = "Originals uploaded and verified".into();
                                if let Some(id) = app.cloud.library {
                                    next = Some(Job::Photos(id, None));
                                }
                            }
                            Outcome::Downloaded(path) => {
                                app.cloud.retry = None;
                                app.cloud.status = "Original downloaded and verified".into();
                                app.cloud.last_download = Some(path);
                            }
                            Outcome::Preview(id, image) => {
                                app.cloud.put_thumbnail(ctx, id, image);
                                app.cloud.status = if app.cloud.preview_errors.contains(&id) {
                                    "No supported embedded preview. Download the original to work with this RAW."
                                } else {
                                    "Image loaded from verified original"
                                }
                                .into();
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(job) = next
        && let Err(e) = app.cloud.start(job)
    {
        app.cloud.error = Some(e);
    }
    if let Ok(progress) = app.cloud.progress_slot.lock() {
        app.cloud.progress = progress.clone();
    }
    if app.cloud.busy {
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}
pub fn enabled(app: &LightcraftApp, id: &str) -> bool {
    let s = &app.cloud;
    match id {
        "cloud.open" | "cloud.local" | "cloud.settings" | "cloud.status" => true,
        "cloud.cancel" => s.busy && s.cancellable,
        "cloud.browser" => s.browser.is_some(),
        "cloud.check" | "cloud.configure" => !s.busy,
        "cloud.login" => !s.busy && s.connected && s.identity.is_none(),
        "cloud.logout" => !s.busy && s.identity.is_some(),
        "cloud.retry" => !s.busy && s.retry.is_some(),
        "cloud.upload" => !s.busy && s.identity.is_some() && s.library.is_some(),
        "cloud.download" | "cloud.preview" => {
            !s.busy && s.identity.is_some() && s.photos.iter().any(|p| Some(p.id) == s.selected && p.status == "ready")
        }
        _ => !s.busy && s.identity.is_some(),
    }
}
fn identifier(p: &Value, key: &str) -> std::result::Result<Uuid, String> {
    p.get(key).and_then(Value::as_str).and_then(|s| Uuid::parse_str(s).ok()).ok_or_else(|| format!("A valid {key} is required."))
}
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> std::result::Result<Value, String> {
    if !enabled(app, id) {
        return Err("This Cloud action is unavailable. Check the connection and sign in, or wait for the current operation.".into());
    }
    let job = match id {
        "cloud.status" => {
            return Ok(
                json!({"connected":app.cloud.connected,"signedIn":app.cloud.identity.is_some(),"remembered":app.cloud.remembered,"vaultWarning":app.cloud.vault_warning,"busy":app.cloud.busy,"status":app.cloud.status,"error":app.cloud.error,"library":app.cloud.library,"libraries":app.cloud.libraries,"photos":app.cloud.photos,"selected":app.cloud.selected,"progress":app.cloud.progress,"thumbnails":app.cloud.thumbnails.keys().map(ToString::to_string).collect::<Vec<_>>() }),
            );
        }
        "cloud.open" => {
            app.cloud.open = true;
            app.ui.fullscreen = false;
            app.ui.dialog = None;
            if !app.cloud.connected && !app.cloud.busy && !app.ui.settings.cloud_server.is_empty() {
                Some(Job::Connect(app.ui.settings.cloud_server.clone()))
            } else {
                None
            }
        }
        "cloud.local" => {
            app.cloud.open = false;
            None
        }
        "cloud.settings" => {
            app.ui.dialog = Some(crate::state::Dialog::Settings { tab: "cloud".into() });
            None
        }
        "cloud.configure" | "cloud.check" => {
            let address = p.get("server").and_then(Value::as_str).unwrap_or(&app.cloud.server_draft);
            let address = lightcraft_cloud_client::server_address(address).map_err(|e| e.to_string())?;
            if app.ui.settings.cloud_server != address {
                app.cloud.clear_account();
                app.cloud.client = None;
                app.cloud.connected = false;
            }
            app.ui.settings.cloud_server = address.clone();
            app.cloud.server_draft = address.clone();
            Some(Job::Connect(address))
        }
        "cloud.login" => Some(Job::Login(app.ui.settings.cloud_remember)),
        "cloud.logout" => Some(Job::Logout),
        "cloud.refresh" => Some(Job::Libraries),
        "cloud.selectLibrary" => {
            let library = identifier(p, "id")?;
            if !app.cloud.libraries.iter().any(|l| l.id == library) {
                return Err("Refresh and select an available library.".into());
            }
            app.cloud.photos.clear();
            app.cloud.selected = None;
            Some(Job::Photos(library, None))
        }
        "cloud.createLibrary" => {
            let name = p.get("name").and_then(Value::as_str).unwrap_or(&app.cloud.new_library).to_owned();
            Some(Job::Create(name))
        }
        "cloud.more" => Some(Job::Photos(app.cloud.library.ok_or("Select a library.")?, app.cloud.next)),
        "cloud.selectPhoto" => {
            let photo = identifier(p, "id")?;
            if !app.cloud.photos.iter().any(|v| v.id == photo) {
                return Err("Select a photo in the current library.".into());
            }
            app.cloud.selected = Some(photo);
            None
        }
        "cloud.cancel" => {
            app.cloud.cancel.store(true, Ordering::Relaxed);
            app.cloud.status = "Cancelling…".into();
            None
        }
        "cloud.retry" => app.cloud.retry.clone(),
        "cloud.browser" => {
            let url = app.cloud.browser.clone().ok_or("No sign-in is waiting.")?;
            app.services.open_url.as_mut().ok_or("System browser is unavailable.")?(&url)?;
            None
        }
        "cloud.upload" => {
            let paths = if let Some(paths) = p.get("paths").and_then(Value::as_array) {
                paths.iter().filter_map(Value::as_str).map(PathBuf::from).collect::<Vec<_>>()
            } else {
                match pick::ask(app, id, p, "paths", PickRequest::files(Some("Upload RAW / JPEG to Cloud".into()), "RAW / JPEG", EXTENSIONS), |s| {
                    s.pick_files.as_mut().map(|f| f())
                }) {
                    Picked::Now(paths) => paths.into_iter().map(PathBuf::from).collect(),
                    Picked::Later => return Ok(Value::Null),
                    Picked::Unavailable => return Err("A native file picker is unavailable; provide paths.".into()),
                }
            };
            if paths.is_empty() {
                return Ok(Value::Null);
            }
            if paths.len() > 128 {
                return Err("Choose at most 128 originals per upload batch.".into());
            }
            Some(Job::Upload(app.cloud.library.ok_or("Select a library.")?, paths.into_iter().map(|p| (p, Uuid::new_v4())).collect()))
        }
        "cloud.preview" | "cloud.download" => {
            let photo = app.cloud.photos.iter().find(|v| Some(v.id) == app.cloud.selected).cloned().ok_or("Select a photo.")?;
            if id == "cloud.preview" {
                Some(Job::Preview(photo))
            } else {
                let path = if let Some(path) = p.get("path").and_then(Value::as_str) {
                    path.to_owned()
                } else {
                    match pick::ask(app, id, p, "path", PickRequest::save("Download original", "Original", EXTENSIONS, &photo.original_name), |_| {
                        None
                    }) {
                        Picked::Now(paths) => paths.first().cloned().unwrap_or_default(),
                        Picked::Later => return Ok(Value::Null),
                        Picked::Unavailable => return Err("A native save dialog is unavailable; provide a path.".into()),
                    }
                };
                if path.is_empty() {
                    return Ok(Value::Null);
                }
                Some(Job::Download(photo, PathBuf::from(path)))
            }
        }
        _ => return Err(format!("Unknown Cloud command: {id}")),
    };
    if let Some(job) = job
        && let Err(e) = app.cloud.start(job)
    {
        app.cloud.error = Some(e.clone());
        return Err(e);
    }
    Ok(Value::Null)
}
