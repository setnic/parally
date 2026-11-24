#![cfg_attr(windows, windows_subsystem = "windows")]

mod bridge;
mod glass;
mod i18n;
mod library;
#[cfg(not(windows))]
#[path = "picker_unix.rs"]
mod picker;
#[cfg(windows)]
mod picker;
mod platform;
#[cfg(windows)]
mod tray_panel;
mod ui;
#[cfg(windows)]
mod window_glass;

use anyhow::{Context as _, Result, ensure};
use eframe::egui::{
    self, Align, CentralPanel, Context, Frame, Layout, Margin, RichText, Ui, ViewportBuilder,
    ViewportCommand, vec2,
};
use i18n::text;
use library::{Entry, Library, Scale, State};
use parallel_downloader::{
    Cancelled, Download, MAX_STREAMS, RestartRequired, StartMode, discard_partial, open_partial,
    validate_url,
};
use picker::{Kind, Picker};
use std::{
    collections::HashMap,
    fs::{self, File},
    path::{Path, PathBuf},
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
#[cfg(windows)]
use std::{
    os::windows::ffi::OsStrExt,
    sync::{
        atomic::{AtomicBool, Ordering::Relaxed},
        mpsc,
    },
};
#[cfg(windows)]
use tray_icon::{
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};
#[cfg(windows)]
use windows_sys::{
    Win32::{
        Foundation::POINT,
        UI::{
            Shell::ShellExecuteW,
            WindowsAndMessaging::{GetCursorPos, MB_ICONERROR, MB_OK, MessageBoxW, SW_SHOWNORMAL},
        },
    },
    w,
};

const TICK: Duration = Duration::from_millis(250);

struct Running {
    engine: Arc<Download>,
    worker: JoinHandle<Result<()>>,
    sample: Instant,
    speed: f64,
    pausing: bool,
    discard: bool,
    browser: Option<bridge::Request>,
}

struct Intake {
    request: bridge::Request,
    destination: String,
    error: String,
}

#[derive(Clone, Copy)]
enum Confirm {
    Cancel(u64),
    Restart(u64),
}

#[cfg(windows)]
enum TrayEvent {
    Toggle(POINT),
    Open,
    Exit,
}

struct App {
    root: PathBuf,
    _lock: File,
    library: Library,
    running: HashMap<u64, Running>,
    last_save: Instant,
    last_poll: Instant,
    url: String,
    destination: String,
    streams: usize,
    notice: String,
    picker: Option<(Picker, bool)>,
    confirm: Option<Confirm>,
    window_geometry: (f32, f32, f32),
    intake: Option<Intake>,
    hidden: bool,
    exiting: bool,
    #[cfg(windows)]
    main_glass: window_glass::GlassWindow,
    #[cfg(windows)]
    tray: Option<TrayIcon>,
    #[cfg(windows)]
    tray_events: mpsc::Receiver<TrayEvent>,
    #[cfg(windows)]
    panel: Option<tray_panel::Panel>,
    #[cfg(windows)]
    panel_open: Arc<AtomicBool>,
    settings_open: bool,
    extension_guide: bool,
    logo: egui::TextureHandle,
    #[cfg(windows)]
    tray_labels: (MenuItem, MenuItem),
}

impl App {
    fn new(
        ctx: &Context,
        logo: egui::TextureHandle,
        root: PathBuf,
        lock: File,
        library: Library,
    ) -> Self {
        i18n::set_russian(library.settings.russian);
        glass::style(ctx, library.settings.light);
        let streams = library.settings.connections;
        #[cfg(windows)]
        let (tray, tray_events, panel, panel_open, tray_labels) = {
            let (send, tray_events) = mpsc::channel();
            let exit = MenuItem::new(text("Exit"), true, None);
            let exit_id = exit.id().clone();
            let open = MenuItem::new(text("Open Window"), true, None);
            let open_id = open.id().clone();
            let menu = Menu::new();
            let wake = ctx.clone();
            let shutdown = send.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                if event.id == exit_id {
                    let _ = shutdown.send(TrayEvent::Exit);
                    wake.request_repaint();
                } else if event.id == open_id {
                    let _ = shutdown.send(TrayEvent::Open);
                    wake.request_repaint();
                }
            }));
            let wake = ctx.clone();
            TrayIconEvent::set_event_handler(Some(move |event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    position,
                    ..
                } = event
                {
                    let _ = send.send(TrayEvent::Toggle(POINT {
                        x: position.x as i32,
                        y: position.y as i32,
                    }));
                    wake.request_repaint();
                }
            }));
            let tray = (|| -> Result<TrayIcon> {
                menu.append(&open)?;
                menu.append(&exit)?;
                let (rgba, size) = glass::tray_icon();
                let icon = tray_icon::Icon::from_rgba(rgba, size, size)?;
                Ok(TrayIconBuilder::new()
                    .with_icon(icon)
                    .with_tooltip("Parally")
                    .with_menu(Box::new(menu))
                    .with_menu_on_left_click(false)
                    .build()?)
            })()
            .ok();
            let panel_open = Arc::new(AtomicBool::new(false));
            let panel = tray_panel::Panel::new(ctx, panel_open.clone()).ok();
            (tray, tray_events, panel, panel_open, (open, exit))
        };
        Self {
            root,
            _lock: lock,
            library,
            running: HashMap::new(),
            last_save: Instant::now(),
            last_poll: Instant::now(),
            url: String::new(),
            destination: String::new(),
            streams,
            notice: String::new(),
            picker: None,
            confirm: None,
            window_geometry: (0.0, 0.0, 0.0),
            intake: None,
            hidden: false,
            exiting: false,
            #[cfg(windows)]
            main_glass: Default::default(),
            #[cfg(windows)]
            tray,
            #[cfg(windows)]
            tray_events,
            #[cfg(windows)]
            panel,
            #[cfg(windows)]
            panel_open,
            settings_open: false,
            extension_guide: false,
            logo,
            #[cfg(windows)]
            tray_labels,
        }
    }

    fn save(&mut self) {
        if let Err(error) = self.library.save(&self.root) {
            self.notice = error_text(&error);
        }
        self.last_save = Instant::now();
    }

    fn show_window(&mut self, ctx: &Context) {
        #[cfg(windows)]
        if let Some(panel) = &mut self.panel {
            panel.hide();
        }
        self.hidden = false;
        #[cfg(windows)]
        self.main_glass.show();
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn has_tray(&self) -> bool {
        #[cfg(windows)]
        return self.tray.is_some();
        #[cfg(not(windows))]
        return false;
    }

    fn hide_window(&mut self, ctx: &Context) {
        if self.intake.is_some() {
            self.cancel_intake();
        }
        if self.has_tray() {
            self.hidden = true;
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        } else {
            self.exit();
        }
    }

    fn exit(&mut self) {
        self.exiting = true;
        self.cancel_intake();
        #[cfg(windows)]
        if let Some(panel) = &mut self.panel {
            panel.hide();
        }
        for transfer in self.running.values_mut() {
            transfer.pausing = transfer.engine.cancel();
        }
        self.notice = text("Saving downloads before exit…").into();
    }

    fn start_new(&mut self, ctx: &Context, browser: bool) -> Result<()> {
        ensure!(!self.exiting, "Parally is closing.");
        let (url, destination) = if browser {
            let draft = self.intake.as_ref().context("Browser request expired")?;
            ensure!(draft.request.live(), "The browser request expired.");
            (&draft.request.offer.url, &draft.destination)
        } else {
            (&self.url, &self.destination)
        };
        let url = validate_url(url)?.to_string();
        let path = library::normalize_destination(Path::new(destination.trim()))?;
        let free = self.library.available_path(&path)?;
        if free != path {
            let draft = if browser {
                &mut self.intake.as_mut().unwrap().destination
            } else {
                &mut self.destination
            };
            *draft = free.to_string_lossy().into_owned();
            anyhow::bail!("That path is in use. Parally suggested a free name. Confirm to use it.");
        }
        let id = self.library.next_id();
        let mut entry = Entry::new(id, url, path, self.streams);
        if browser {
            entry.headers = self.intake.as_ref().unwrap().request.offer.headers();
        }
        self.library.entries.push(entry);
        let request = if browser {
            self.intake.take().map(|draft| draft.request)
        } else {
            None
        };
        self.launch(ctx, id, request, StartMode::Normal)?;
        if !browser {
            self.url.clear();
            self.destination.clear();
        }
        Ok(())
    }

    fn launch(
        &mut self,
        ctx: &Context,
        id: u64,
        request: Option<bridge::Request>,
        mode: StartMode,
    ) -> Result<()> {
        let result = (|| {
            ensure!(!self.exiting, "Parally is closing.");
            let path = &self
                .library
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .context("Download is unavailable")?
                .path;
            ensure!(
                !self
                    .library
                    .entries
                    .iter()
                    .any(|entry| entry.state == State::Active
                        && entry.id != id
                        && library::same_path(path, &entry.path)),
                "Another download is using this destination."
            );
            let entry = self
                .library
                .entries
                .iter_mut()
                .find(|entry| entry.id == id)
                .context("Download is unavailable")?;
            ensure!(
                !library::exists(&entry.path)?,
                "The destination exists. Move the existing file or start a new download with another name."
            );
            ensure!(
                !self.running.contains_key(&id),
                "This download is already active."
            );
            ensure!(
                !entry.canceled && entry.state != State::Complete,
                "This download is already closed."
            );
            ensure!(
                library::valid_streams(entry.streams),
                "Choose a valid number of connections."
            );
            let (url, path, streams) = (entry.url.clone(), entry.path.clone(), entry.streams);
            let headers = entry.headers.clone();
            let previous = entry.state;
            entry.state = State::Active;
            if let Err(error) = self.library.save(&self.root) {
                self.library
                    .entries
                    .iter_mut()
                    .find(|entry| entry.id == id)
                    .unwrap()
                    .state = previous;
                return Err(error);
            }
            let engine = Arc::new(Download::with_mode(mode).with_headers(headers));
            let job = engine.clone();
            let wake = ctx.clone();
            let worker =
                match thread::Builder::new()
                    .name(format!("parally-{id}"))
                    .spawn(move || {
                        let result = job.run(&url, &path, streams);
                        wake.request_repaint();
                        result
                    }) {
                    Ok(worker) => worker,
                    Err(error) => {
                        let entry = self
                            .library
                            .entries
                            .iter_mut()
                            .find(|entry| entry.id == id)
                            .unwrap();
                        entry.state = State::Interrupted;
                        entry.detail = "Could not start this download.".into();
                        self.save();
                        return Err(error).context("Could not start this download");
                    }
                };
            Ok((engine, worker))
        })();
        match result {
            Ok((engine, worker)) => {
                // A paused browser download waits for the first bytes, so the browser can resume it if Parally fails.
                let request = request.filter(|request| {
                    if request.offer.manual {
                        let _ = request.reply(true);
                        false
                    } else {
                        true
                    }
                });
                self.running.insert(
                    id,
                    Running {
                        engine,
                        worker,
                        sample: Instant::now(),
                        speed: 0.0,
                        pausing: false,
                        discard: false,
                        browser: request,
                    },
                );
                self.notice.clear();
                Ok(())
            }
            Err(error) => {
                if let Some(request) = request {
                    let _ = request.reply(false);
                }
                Err(error)
            }
        }
    }

    fn poll_jobs(&mut self) {
        let mut finished = Vec::new();
        for entry in &mut self.library.entries {
            let Some(transfer) = self.running.get_mut(&entry.id) else {
                continue;
            };
            if transfer
                .browser
                .as_ref()
                .is_some_and(|request| !request.live())
            {
                transfer.browser = None;
                transfer.pausing = transfer.engine.cancel();
            }
            if transfer.worker.is_finished() {
                finished.push(entry.id);
                continue;
            }
            let progress = transfer.engine.progress();
            let elapsed = transfer.sample.elapsed().as_secs_f64();
            transfer.speed = if transfer.pausing || progress.status != entry.detail {
                0.0
            } else {
                progress.bytes.saturating_sub(entry.bytes) as f64 / elapsed.max(0.001)
            };
            entry.detail.clone_from(&progress.status);
            if progress.status.starts_with("Downloading") {
                entry.restart_required = false;
            }
            entry.observe(progress);
            transfer.sample = Instant::now();
            if entry.bytes > 0
                && !transfer.pausing
                && let Some(request) = transfer.browser.take()
                && !request.reply(true).is_ok_and(|sent| sent)
            {
                transfer.pausing = transfer.engine.cancel();
            }
        }
        let changed = !finished.is_empty();
        for id in finished {
            let transfer = self.running.remove(&id).unwrap();
            let progress = transfer.engine.progress();
            let entry = self
                .library
                .entries
                .iter_mut()
                .find(|entry| entry.id == id)
                .unwrap();
            let received = progress.bytes;
            if progress.status.starts_with("Downloading") {
                entry.restart_required = false;
            }
            entry.observe(progress);
            let result = transfer.worker.join().unwrap_or_else(|_| {
                Err(anyhow::anyhow!("A download worker stopped unexpectedly."))
            });
            if let Some(request) = transfer.browser {
                let _ = request.reply(result.is_ok() && received > 0);
            }
            match result {
                Ok(()) => {
                    entry.state = State::Complete;
                    entry.detail.clear();
                    entry.saved = false;
                    entry.restart_required = false;
                    entry.segments.clear();
                }
                Err(error) => {
                    entry.state = if error.is::<Cancelled>() {
                        State::Paused
                    } else {
                        State::Interrupted
                    };
                    entry.detail = if error.is::<Cancelled>() {
                        String::new()
                    } else {
                        error_text(&error)
                    };
                    entry.restart_required |= error.is::<RestartRequired>();
                    entry.read_checkpoint();
                    if transfer.discard
                        && let Err(error) = cancel_entry(entry)
                    {
                        self.notice = error_text(&error);
                    }
                }
            }
        }
        if changed || !self.running.is_empty() && self.last_save.elapsed() >= Duration::from_secs(2)
        {
            self.save();
        }
    }

    fn show_offer(&mut self, ctx: &Context) {
        if self
            .intake
            .as_ref()
            .is_some_and(|draft| !draft.request.live())
        {
            self.cancel_intake();
        }
        if self.exiting || self.intake.is_some() || self.picker.is_some() || self.confirm.is_some()
        {
            return;
        }
        if let Some(request) = bridge::offers(&self.root).into_iter().find(|request| {
            !self.running.values().any(|running| {
                running
                    .browser
                    .as_ref()
                    .is_some_and(|active| active.path == request.path)
            })
        }) {
            let fallback = library::suggested_name(&request.offer.url);
            let name = request
                .offer
                .filename
                .rsplit(['/', '\\'])
                .next()
                .filter(|name| library::valid_filename(name))
                .unwrap_or(&fallback);
            let folder = self.current_folder().cloned().or_else(default_folder);
            let destination = folder.map_or_else(String::new, |folder| {
                folder.join(name).to_string_lossy().into_owned()
            });
            self.intake = Some(Intake {
                request,
                destination,
                error: String::new(),
            });
            self.show_window(ctx);
        }
    }

    fn cancel_intake(&mut self) {
        if let Some(draft) = self.intake.take() {
            let _ = draft.request.reply(false);
        }
        if self.picker.as_ref().is_some_and(|(_, browser)| *browser) {
            self.picker = None;
        }
    }

    fn current_folder(&self) -> Option<&PathBuf> {
        self.library
            .last_folder
            .and_then(|index| self.library.folders.get(index))
    }

    fn set_folder(&mut self, index: usize, browser: bool) {
        let Some(folder) = self.library.folders.get(index) else {
            return;
        };
        self.library.last_folder = Some(index);
        let (destination, url) = if browser {
            let draft = self.intake.as_mut().unwrap();
            (&mut draft.destination, &draft.request.offer.url)
        } else {
            (&mut self.destination, &self.url)
        };
        let fallback = library::suggested_name(url);
        let name = Path::new(destination)
            .file_name()
            .unwrap_or(fallback.as_ref());
        *destination = folder.join(name).to_string_lossy().into_owned();
        self.save();
    }

    fn folder_picker(&mut self, browser: bool) {
        let start = self
            .current_folder()
            .map(|path| path.to_string_lossy())
            .unwrap_or_default();
        self.picker = Some((Picker::new(Kind::Folder, &start, ""), browser));
    }

    fn preset_control(&mut self, ui: &mut Ui, browser: bool) {
        let mut selection = None;
        let mut add = false;
        let selected = self
            .current_folder()
            .map(|folder| folder_label(folder))
            .unwrap_or_else(|| text("Choose folder").into());
        ui.label(text("Folder preset"));
        egui::ComboBox::from_id_salt(("folder-presets", browser))
            .width(156.0)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for (index, folder) in self.library.folders.iter().enumerate() {
                    if ui
                        .selectable_label(
                            self.library.last_folder == Some(index),
                            folder_label(folder),
                        )
                        .on_hover_text(folder.to_string_lossy())
                        .clicked()
                    {
                        selection = Some(index);
                    }
                }
                if ui.selectable_label(false, text("Add folder…")).clicked() {
                    add = true;
                }
            });
        if let Some(index) = selection {
            self.set_folder(index, browser);
        }
        if add {
            self.folder_picker(browser);
        }
    }

    fn restore(&mut self, path: &Path) -> Result<()> {
        let (url, destination, streams) = open_partial(path)?;
        let destination = library::normalize_destination(&destination)?;
        if let Some(entry) = self.library.entries.iter().find(|entry| {
            library::same_path(&entry.path, &destination)
                && entry.state != State::Complete
                && !entry.canceled
        }) {
            self.notice = format!("{} is in Downloads.", filename(&entry.path));
            return Ok(());
        }
        let mut entry = Entry::new(self.library.next_id(), url, destination, streams);
        entry.state = State::Paused;
        entry.read_checkpoint();
        self.library.entries.push(entry);
        self.notice = "Partial download added.".into();
        self.save();
        Ok(())
    }

    fn show_picker(&mut self) {
        if let Some((picker, browser)) = self.picker.take() {
            #[cfg(windows)]
            let selection = picker.show(self.main_glass.handle());
            #[cfg(not(windows))]
            let selection = picker.show();
            let outcome = selection.and_then(|selection| {
                let Some(path) = selection else { return Ok(()) };
                match picker.kind {
                    Kind::Partial => self.restore(&path),
                    Kind::Save => {
                        let text = path.to_string_lossy().into_owned();
                        if browser {
                            if let Some(draft) = &mut self.intake {
                                draft.destination = text;
                            }
                        } else {
                            self.destination = text;
                        }
                        Ok(())
                    }
                    Kind::Folder => self
                        .library
                        .add_folder(path)
                        .map(|index| self.set_folder(index, browser)),
                }
            });
            if let Err(error) = outcome {
                if browser && let Some(draft) = &mut self.intake {
                    draft.error = error_text(&error);
                } else {
                    self.notice = error_text(&error);
                }
            }
        }
    }

    fn cancel_download(&mut self, id: u64) -> Result<()> {
        let entry = self
            .library
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .context("Download is unavailable")?;
        ensure!(
            entry.state != State::Complete && !entry.canceled,
            "This download is already closed."
        );
        ensure!(
            !self.library.entries.iter().any(|other| other.id != id
                && (other.id > id || other.state == State::Active)
                && library::same_path(&other.path, &entry.path)),
            "Another download owns this destination."
        );
        if let Some(job) = self.running.get_mut(&id) {
            ensure!(job.engine.cancel(), "The file has already finished.");
            job.discard = true;
            job.pausing = true;
        } else {
            cancel_entry(
                self.library
                    .entries
                    .iter_mut()
                    .find(|entry| entry.id == id)
                    .unwrap(),
            )?;
            self.save();
        }
        Ok(())
    }

    #[cfg(windows)]
    fn toggle_panel(&mut self, ctx: &Context, point: POINT) {
        if let Some(panel) = &mut self.panel {
            panel.toggle(point, monitor_scale(ctx));
        } else {
            self.show_window(ctx);
        }
    }

    #[cfg(windows)]
    fn refresh_panel(&mut self) {
        let Some(panel) = &mut self.panel else { return };
        let (percent, text) = if let Some(entry) = self
            .library
            .entries
            .iter()
            .find(|entry| self.running.contains_key(&entry.id))
        {
            let active = self.running.len();
            let progress = fraction(entry).map(|value| (value * 100.0).round() as u8);
            let speed = self.running.get(&entry.id).map_or(0.0, |job| job.speed);
            let status = match progress {
                Some(value) => format!("{value}%"),
                None => size(entry.bytes as f64),
            };
            let text = if active == 1 {
                format!("{status}  ·  {}/s", size(speed))
            } else {
                format!("{active} {}  ·  {}/s", text("active"), size(speed))
            };
            (progress, text)
        } else {
            (None, text("No active downloads").into())
        };
        if let Err(error) = panel.refresh(
            percent,
            text,
            self.library.settings.transparency,
            self.library.settings.light,
        ) {
            panel.hide();
            self.notice = error_text(&error);
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let zoom = self.library.scale.zoom(monitor_scale(ctx));
        if ctx.zoom_factor() != zoom {
            ctx.set_zoom_factor(zoom);
        }
        #[cfg(windows)]
        self.main_glass.update_main(_frame, ctx.pixels_per_point());
        ctx.request_repaint_after(TICK);
        #[cfg(windows)]
        while let Ok(event) = self.tray_events.try_recv() {
            match event {
                TrayEvent::Toggle(point) if !self.exiting => self.toggle_panel(ctx, point),
                TrayEvent::Open if !self.exiting => self.show_window(ctx),
                TrayEvent::Exit => self.exit(),
                _ => {}
            }
        }
        #[cfg(windows)]
        let panel_open = self.panel_open.swap(false, Relaxed);
        #[cfg(not(windows))]
        let panel_open = false;
        if panel_open || fs::remove_file(self.root.join("show-window")).is_ok() {
            self.show_window(ctx);
        }
        if ctx.input(|input| input.viewport().close_requested())
            && (!self.exiting || !self.running.is_empty())
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            if !self.exiting {
                self.hide_window(ctx);
            }
        }
        if self.last_poll.elapsed() >= TICK {
            self.last_poll = Instant::now();
            self.poll_jobs();
            self.show_offer(ctx);
        }
        if self.exiting && self.running.is_empty() {
            self.save();
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        #[cfg(windows)]
        self.refresh_panel();
        if !self.hidden {
            CentralPanel::default().frame(Frame::NONE).show(ctx, |ui| {
                glass::background(ui, self.library.settings.transparency);
                Frame::new()
                    .inner_margin(Margin::symmetric(24, 16))
                    .show(ui, |ui| self.main_ui(ui, ctx));
            });
            self.intake_ui(ctx);
            self.confirm_ui(ctx);
            self.settings_ui(ctx);
            self.extension_ui(ctx);
            self.show_picker();
        }
    }
}

fn monitor_scale(ctx: &Context) -> f32 {
    ctx.input(|input| input.viewport().native_pixels_per_point)
        .unwrap_or(1.0)
        .max(0.25)
}

const ROW_HEIGHT: f32 = 82.0;
const ROW_GAP: f32 = 6.0;

fn fit_rows(count: usize, available: f32) -> f32 {
    let available = available.max(1.0);
    if count == 0 {
        return available.min(29.0);
    }
    let fit = ((available + ROW_GAP) / (ROW_HEIGHT + ROW_GAP)).floor() as usize;
    let rows = count.min(3).min(fit.max(1)) as f32;
    available.min(rows * (ROW_HEIGHT + ROW_GAP) - ROW_GAP)
}

fn cancel_entry(entry: &mut Entry) -> Result<()> {
    discard_partial(&entry.path, &entry.url)?;
    entry.state = State::Interrupted;
    entry.canceled = true;
    entry.saved = false;
    entry.restart_required = false;
    entry.bytes = 0;
    entry.total = None;
    entry.segments.clear();
    entry.detail.clear();
    Ok(())
}

fn filename(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}
fn folder_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}
fn default_folder() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| home.join("Downloads"))
        .filter(|folder| folder.is_dir())
}
fn suggested_browser_name(draft: &Intake) -> String {
    let name = filename(Path::new(&draft.destination));
    if name.is_empty() {
        library::suggested_name(&draft.request.offer.url)
    } else {
        name
    }
}
fn fraction(entry: &Entry) -> Option<f32> {
    if entry.state == State::Complete {
        Some(1.0)
    } else {
        entry
            .total
            .filter(|total| *total > 0)
            .map(|total| (entry.bytes as f64 / total as f64).clamp(0.0, 1.0) as f32)
    }
}
fn size(bytes: f64) -> String {
    let mut value = bytes;
    for unit in ["B", "KiB", "MiB", "GiB", "TiB"] {
        if value < 1024.0 || unit == "TiB" {
            return if unit == "B" {
                format!("{value:.0} B")
            } else {
                format!("{value:.1} {unit}")
            };
        }
        value /= 1024.0;
    }
    unreachable!()
}
fn error_text(error: &anyhow::Error) -> String {
    #[cfg(windows)]
    if let Some(code) = error
        .downcast_ref::<std::io::Error>()
        .and_then(std::io::Error::raw_os_error)
    {
        if error.chain().count() == 1 {
            return format!("Windows could not access the file (error {code}).");
        }
        return format!("{error} (Windows error {code}).");
    }
    format!("{error:#}")
}
fn open_path(path: &Path) -> Result<()> {
    ensure!(
        library::exists(path)?,
        "The file or folder is no longer at this location."
    );
    #[cfg(not(windows))]
    return platform::open_path(path);
    #[cfg(windows)]
    {
        let path: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                w!("open"),
                path.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        ensure!(
            result as isize > 32,
            "Windows could not open the file or folder."
        );
        Ok(())
    }
}

fn app(browser: bool) -> Result<()> {
    let root = library::root()?;
    let Some(lock) = library::try_instance_lock(&root)? else {
        if !browser {
            fs::write(root.join("show-window"), [])?;
        }
        return Ok(());
    };
    let library = Library::load(&root)?;
    // The extension finds Parally through this registration; the settings guide reports errors.
    thread::spawn(|| platform::register_browsers(true));
    let logo = glass::logo();
    let icon = glass::icon_data(&logo);
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("Parally")
            .with_icon(icon)
            .with_inner_size([735.0, 350.0])
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(true),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Parally",
        options,
        Box::new(move |cc| {
            let logo = egui::ColorImage::from_rgba_unmultiplied(
                [logo.width() as usize, logo.height() as usize],
                logo.as_raw(),
            );
            let texture =
                cc.egui_ctx
                    .load_texture("Parally logo", logo, egui::TextureOptions::LINEAR);
            Ok(Box::new(App::new(
                &cc.egui_ctx,
                texture,
                root,
                lock,
                library,
            )))
        }),
    )
    .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(())
}

fn main() {
    let argument = std::env::args().nth(1).unwrap_or_default();
    if bridge::native_origin(&argument) {
        bridge::native_host();
    } else if argument == "--disconnect-browsers" {
        let _ = platform::register_browsers(false);
    } else if let Err(error) = app(argument == "--browser-request") {
        #[cfg(not(windows))]
        eprintln!("Parally could not open. {}", error_text(&error));
        #[cfg(windows)]
        {
            let text: Vec<_> = format!("Parally could not open.\n\n{}", error_text(&error))
                .encode_utf16()
                .chain([0])
                .collect();
            unsafe {
                MessageBoxW(
                    std::ptr::null_mut(),
                    text.as_ptr(),
                    w!("Parally"),
                    MB_OK | MB_ICONERROR,
                )
            };
        }
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    #[test]
    fn fits_at_most_three_complete_rows() {
        assert_eq!(
            [0, 1, 2, 3, 5].map(|count| fit_rows(count, 600.0)),
            [29.0, 82.0, 170.0, 258.0, 258.0]
        );
        assert_eq!(fit_rows(5, 200.0), 170.0);
        assert_eq!(fit_rows(5, 120.0), 82.0);
    }
}
