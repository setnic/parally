//! Window panels: intake, main list, settings and confirmation dialogs.
use super::*;

impl App {
    pub(super) fn intake_ui(&mut self, ctx: &Context) {
        if self.intake.is_none() || self.picker.is_some() {
            return;
        }
        let mut accept = false;
        let mut cancel = false;
        let mut browse = false;
        let modal = egui::Modal::new(egui::Id::new("browser-destination"))
            .frame(glass::card().fill(glass::popup()))
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 64.0).clamp(300.0, 495.0));
                ui.label(RichText::new(text("Browser download")).size(21.0).strong());
                let draft = self.intake.as_ref().unwrap();
                let name = suggested_browser_name(draft);
                ui.add(egui::Label::new(name).truncate());
                ui.add_space(9.0);
                ui.horizontal(|ui| self.preset_control(ui, true));
                let draft = self.intake.as_mut().unwrap();
                ui.label(text("Save to"));
                ui.horizontal(|ui| {
                    glass::field(
                        ui,
                        "browser-path",
                        &mut draft.destination,
                        "Choose a folder preset or enter a full path",
                        ui.available_width() - 102.0,
                    );
                    browse = ui.add(glass::button("Browse…", false, 94.0)).clicked();
                });
                if !draft.error.is_empty() {
                    ui.label(text(&draft.error));
                }
                ui.add_space(8.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    accept = ui
                        .add(glass::button("Start download", true, 125.0))
                        .clicked();
                    cancel = ui
                        .add(glass::button(
                            if draft.request.offer.manual {
                                "Cancel"
                            } else {
                                "Keep in browser"
                            },
                            false,
                            125.0,
                        ))
                        .clicked();
                });
            });
        if cancel || modal.should_close() {
            self.cancel_intake();
        } else if browse {
            let draft = self.intake.as_ref().unwrap();
            self.picker = Some((
                Picker::new(
                    Kind::Save,
                    &draft.destination,
                    &suggested_browser_name(draft),
                ),
                true,
            ));
        } else if accept && let Err(error) = self.start_new(ctx, true) {
            if let Some(draft) = &mut self.intake {
                draft.error = error_text(&error);
            } else {
                self.notice = error_text(&error);
            }
        }
    }

    pub(super) fn main_ui(&mut self, ui: &mut Ui, ctx: &Context) {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let title = ui.allocate_ui_with_layout(
                vec2((ui.available_width() - 175.0).max(125.0), 36.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_min_size(ui.available_size());
                    ui.image((self.logo.id(), vec2(32.0, 32.0)));
                    ui.add(
                        egui::Label::new(RichText::new("Parally").size(23.0).strong())
                            .selectable(false),
                    );
                },
            );
            if ui
                .interact(
                    title.response.rect,
                    ui.id().with("title-drag"),
                    egui::Sense::drag(),
                )
                .drag_started()
            {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if ui.add(glass::button("Settings", false, 86.0)).clicked() {
                self.settings_open = true;
            }
            if glass::window_button(ui, "−", "Minimize").clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
            }
            if glass::window_button(
                ui,
                "×",
                if self.has_tray() {
                    "Hide to tray"
                } else {
                    "Close"
                },
            )
            .clicked()
            {
                self.hide_window(ctx);
            }
        });
        ui.add_space(8.0);
        glass::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_enabled_ui(!self.exiting, |ui| {
                ui.label(text("Download URL"));
                let auto_destination = self.destination.is_empty()
                    || self.current_folder().is_some_and(|folder| {
                        folder.join(library::suggested_name(&self.url))
                            == Path::new(&self.destination)
                    });
                let link = glass::field(
                    ui,
                    "url",
                    &mut self.url,
                    "https://example.com/file.zip",
                    ui.available_width(),
                );
                if link.changed()
                    && auto_destination
                    && validate_url(&self.url).is_ok()
                    && let Some(folder) = self.current_folder()
                {
                    self.destination = folder
                        .join(library::suggested_name(&self.url))
                        .to_string_lossy()
                        .into_owned();
                }
                ui.label(text("Save to"));
                ui.horizontal(|ui| {
                    glass::field(
                        ui,
                        "destination",
                        &mut self.destination,
                        "Choose a destination file",
                        ui.available_width() - 102.0,
                    );
                    if ui.add(glass::button("Browse…", false, 94.0)).clicked() {
                        self.picker = Some((
                            Picker::new(
                                Kind::Save,
                                &self.destination,
                                &library::suggested_name(&self.url),
                            ),
                            false,
                        ));
                    }
                });
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 34.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.spacing_mut().interact_size.y = 34.0;
                        self.preset_control(ui, false);
                        ui.add_space(6.0);
                        ui.label(text("Connections"));
                        let original_streams = self.streams;
                        egui::ComboBox::from_id_salt("connections")
                            .selected_text(self.streams.to_string())
                            .width(54.0)
                            .show_ui(ui, |ui| {
                                for value in 1..=MAX_STREAMS {
                                    ui.selectable_value(
                                        &mut self.streams,
                                        value,
                                        value.to_string(),
                                    );
                                }
                            });
                        if self.streams != original_streams {
                            self.library.settings.connections = self.streams;
                            for entry in &mut self.library.entries {
                                if entry.state != State::Complete && !entry.canceled {
                                    entry.streams = self.streams;
                                    if let Some(job) = self.running.get(&entry.id) {
                                        job.engine.set_connections(self.streams);
                                    }
                                }
                            }
                            self.save();
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(glass::button("Download", true, 100.0)).clicked()
                                && let Err(error) = self.start_new(ctx, false)
                            {
                                self.notice = error_text(&error);
                            }
                        });
                    },
                );
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(text("Downloads")).size(18.0).strong());
        });
        if !self.notice.is_empty() {
            ui.label(text(&self.notice));
        }
        let scale = ctx.pixels_per_point();
        #[cfg(windows)]
        let (work_width, work_height) = self.main_glass.work_size(scale);
        #[cfg(not(windows))]
        let (work_width, work_height) = ctx
            .input(|input| input.viewport().monitor_size)
            .map_or((735.0, 730.0), |size| {
                ((size.x - 24.0).max(400.0), (size.y - 48.0).max(240.0))
            });
        let header = ui.next_widget_position().y + 18.0;
        let list_height = fit_rows(self.library.entries.len(), work_height - header);
        let minimum_height: f32 = if self.settings_open { 424.0 } else { 0.0 };
        let (width, height) = (
            735.0_f32.min(work_width).floor(),
            (header + list_height)
                .ceil()
                .max(minimum_height)
                .min(work_height.floor()),
        );
        let geometry = (width, height, scale);
        if self.window_geometry != geometry {
            self.window_geometry = geometry;
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(width, height)));
        }
        self.download_list(ui, ctx, list_height);
    }

    pub(super) fn settings_ui(&mut self, ctx: &Context) {
        if !self.settings_open || self.intake.is_some() || self.confirm.is_some() {
            return;
        }
        let previous = self.library.settings;
        let previous_scale = self.library.scale;
        let settings = &mut self.library.settings;
        let modal = egui::Modal::new(egui::Id::new("preferences"))
            .frame(glass::card().fill(glass::popup()))
            .show(ctx, |ui| {
                ui.set_width(405.0);
                ui.heading(text("Settings"));
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 123.0).max(96.0))
                    .show(ui, |ui| {
                        egui::Grid::new("preferences-grid").num_columns(2).spacing(vec2(18.0, 10.0)).show(ui, |ui| {
                    ui.label(text("Browser"));
                    if ui.button(text("Add Parally to a browser…")).clicked() { self.extension_guide = true; }
                    ui.end_row();
                    ui.label(text("Startup"));
                    ui.checkbox(&mut settings.start_at_login, text("Start Parally at login"));
                    ui.end_row();
                    ui.label(text("Transparency"));
                    ui.add(egui::Slider::new(&mut settings.transparency, 0..=45).suffix("%"));
                    ui.end_row();
                    ui.label(text("Theme"));
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut settings.light, false, text("Dark"));
                        ui.selectable_value(&mut settings.light, true, text("Light"));
                    });
                    ui.end_row();
                    ui.label(text("Language"));
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut settings.russian, false, "English");
                        ui.selectable_value(&mut settings.russian, true, "Русский");
                    });
                    ui.end_row();
                    ui.label(text("Scale"));
                    egui::ComboBox::from_id_salt("ui-scale").selected_text(text(self.library.scale.label())).show_ui(ui, |ui| {
                        for option in Scale::OPTIONS {
                            ui.selectable_value(&mut self.library.scale, option, text(option.label()));
                        }
                    });
                    ui.end_row();
                        });
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add(glass::button("Done", true, 88.0)).clicked()
                    }).inner
                }).inner
            });
        if previous.start_at_login != settings.start_at_login
            && let Err(error) = platform::set_startup(settings.start_at_login)
        {
            settings.start_at_login = previous.start_at_login;
            self.notice = error_text(&error);
        }
        if previous.russian != settings.russian {
            i18n::set_russian(settings.russian);
            #[cfg(windows)]
            {
                self.tray_labels.0.set_text(text("Open Window"));
                self.tray_labels.1.set_text(text("Exit"));
            }
        }
        if previous.light != settings.light {
            glass::style(ctx, settings.light);
        }
        if previous != *settings || previous_scale != self.library.scale {
            self.save();
        }
        if self.extension_guide {
            self.settings_open = false;
        }
        if modal.inner || modal.should_close() {
            self.settings_open = false;
        }
    }

    pub(super) fn extension_ui(&mut self, ctx: &Context) {
        if !self.extension_guide {
            return;
        }
        let mut open_folder = false;
        let modal = egui::Modal::new(egui::Id::new("browser-extension"))
            .frame(glass::card().fill(glass::popup()))
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 64.0).clamp(300.0, 495.0));
                ui.label(RichText::new(text("Add Parally to a browser")).size(21.0).strong());
                ui.add_space(6.0);
                for step in [
                    "1. Open the browser's extensions page, such as chrome://extensions or edge://extensions.",
                    "2. Turn on Developer mode.",
                    "3. Click Load unpacked and choose the folder Parally opens. Its path is copied.",
                ] {
                    ui.label(text(step));
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(text(
                        "Downloads then open in Parally. Click the Parally button in the browser toolbar to turn this off.",
                    ))
                    .color(glass::muted()),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    open_folder = ui
                        .add(glass::button("Open extension folder", true, 180.0))
                        .clicked();
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add(glass::button("Done", false, 88.0)).clicked()
                    })
                    .inner
                })
                .inner
            });
        if open_folder {
            let opened = platform::register_browsers(true).and_then(|()| {
                let folder = platform::extension_folder()?;
                ctx.copy_text(folder.to_string_lossy().into_owned());
                open_path(&folder)
            });
            if let Err(error) = opened {
                self.notice = error_text(&error);
                self.extension_guide = false;
            }
        }
        if modal.inner || modal.should_close() {
            self.extension_guide = false;
        }
    }

    fn download_list(&mut self, ui: &mut Ui, ctx: &Context, list_height: f32) {
        enum Action {
            Pause(u64),
            Resume(u64),
            Confirm(Confirm),
            Open(PathBuf),
        }
        let mut action = None;
        let count = self.library.entries.len();
        if count == 0 {
            ui.label(text("No downloads yet."));
            return;
        }
        egui::ScrollArea::vertical()
            .max_height(list_height)
            .auto_shrink([false, false])
            .show_rows(ui, ROW_HEIGHT, count, |ui, rows| {
                for row in rows {
                    let entry = &self.library.entries[count - row - 1];
                    ui.push_id(entry.id, |ui| {
                        glass::card()
                            .inner_margin(Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.set_min_height(ROW_HEIGHT - 18.0);
                                ui.spacing_mut().item_spacing.y = 4.0;
                                ui.spacing_mut().button_padding.y = 3.0;
                                let transfer = self.running.get(&entry.id);
                                let resume = entry.saved && !entry.restart_required;
                                let state = match entry.state {
                                    _ if entry.canceled => "Canceled",
                                    State::Active if transfer.is_some_and(|job| job.discard) => {
                                        "Canceling…"
                                    }
                                    State::Active if transfer.is_some_and(|job| job.pausing) => {
                                        "Pausing…"
                                    }
                                    State::Active if entry.detail.is_empty() => "Connecting…",
                                    State::Active => {
                                        entry.detail.split(" · ").next().unwrap_or("Downloading")
                                    }
                                    State::Paused => "Paused",
                                    State::Interrupted if entry.restart_required => {
                                        "Restart required"
                                    }
                                    State::Interrupted => "Interrupted",
                                    State::Complete => "Complete",
                                };
                                ui.horizontal(|ui| {
                                    ui.allocate_ui_with_layout(
                                        vec2((ui.available_width() - 212.0).max(80.0), 26.0),
                                        Layout::left_to_right(Align::Center),
                                        |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(filename(&entry.path))
                                                        .strong()
                                                        .size(16.0),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(entry.path.to_string_lossy());
                                        },
                                    );
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if entry.state == State::Complete {
                                            if ui
                                                .add(
                                                    glass::button("Open Folder", false, 105.0)
                                                        .min_size(vec2(105.0, 26.0)),
                                                )
                                                .clicked()
                                                && let Some(parent) = entry.path.parent()
                                            {
                                                action = Some(Action::Open(parent.to_owned()));
                                            }
                                            if ui
                                                .add(
                                                    glass::button("Open File", false, 88.0)
                                                        .min_size(vec2(88.0, 26.0)),
                                                )
                                                .clicked()
                                            {
                                                action = Some(Action::Open(entry.path.clone()));
                                            }
                                        } else if !entry.canceled {
                                            if ui
                                                .add_enabled(
                                                    !self.exiting
                                                        && !transfer.is_some_and(|job| job.discard),
                                                    glass::button("Cancel", false, 76.0)
                                                        .min_size(vec2(76.0, 26.0)),
                                                )
                                                .clicked()
                                            {
                                                action = Some(Action::Confirm(Confirm::Cancel(
                                                    entry.id,
                                                )));
                                            }
                                            if let Some(transfer) = transfer {
                                                if ui
                                                    .add_enabled(
                                                        !transfer.pausing && !self.exiting,
                                                        glass::button("Pause", false, 76.0)
                                                            .min_size(vec2(76.0, 26.0)),
                                                    )
                                                    .clicked()
                                                {
                                                    action = Some(Action::Pause(entry.id));
                                                }
                                            } else if ui
                                                .add_enabled(
                                                    !self.exiting,
                                                    glass::button(
                                                        if resume { "Resume" } else { "Restart" },
                                                        false,
                                                        83.0,
                                                    )
                                                    .min_size(vec2(83.0, 26.0)),
                                                )
                                                .clicked()
                                            {
                                                action = Some(if resume {
                                                    Action::Resume(entry.id)
                                                } else {
                                                    Action::Confirm(Confirm::Restart(entry.id))
                                                });
                                            }
                                        }
                                    });
                                });
                                let volume = entry.total.map_or_else(
                                    || size(entry.bytes as f64),
                                    |total| {
                                        format!(
                                            "{} / {}",
                                            size(entry.bytes as f64),
                                            size(total as f64)
                                        )
                                    },
                                );
                                let detail = match entry.state {
                                    _ if entry.canceled => text("Partial data deleted.").into(),
                                    State::Active => {
                                        let settings = entry
                                            .detail
                                            .split_once(" · ")
                                            .map_or("", |(_, detail)| detail);
                                        let speed = size(transfer.map_or(0.0, |job| job.speed));
                                        if settings.is_empty() {
                                            format!("{volume}  ·  {speed}/s")
                                        } else {
                                            format!(
                                                "{volume}  ·  {speed}/s  ·  {}",
                                                settings
                                                    .strip_suffix(" active connections")
                                                    .map_or_else(
                                                        || text(settings).to_owned(),
                                                        |count| format!(
                                                            "{count} {}",
                                                            text("active connections")
                                                        )
                                                    )
                                            )
                                        }
                                    }
                                    State::Complete => format!("{volume} {}", text("saved")),
                                    State::Interrupted if !entry.detail.is_empty() => {
                                        format!("{volume}  ·  {}", text(&entry.detail))
                                    }
                                    _ if resume => {
                                        format!(
                                            "{volume} {}  ·  {}",
                                            text("saved"),
                                            text("Server version checked on resume")
                                        )
                                    }
                                    _ => {
                                        format!(
                                            "{volume} {}  ·  {}",
                                            text("received"),
                                            text("Restart from the beginning")
                                        )
                                    }
                                };
                                let percent = fraction(entry);
                                ui.horizontal(|ui| {
                                    let text = format!("{}  ·  {detail}", text(state));
                                    ui.allocate_ui_with_layout(
                                        vec2((ui.available_width() - 53.0).max(10.0), 17.0),
                                        Layout::left_to_right(Align::Center),
                                        |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&text)
                                                        .size(13.0)
                                                        .color(glass::muted()),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(&text);
                                        },
                                    );
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if let Some(value) = percent {
                                            ui.label(format!("{:.0}%", value * 100.0));
                                        }
                                    });
                                });
                                glass::progress(
                                    ui,
                                    percent,
                                    entry.state == State::Active,
                                    &entry.segments,
                                );
                            })
                    });
                }
            });
        let result = match action {
            Some(Action::Pause(id)) => {
                if let Some(job) = self.running.get_mut(&id) {
                    job.pausing = job.engine.cancel();
                }
                Ok(())
            }
            Some(Action::Resume(id)) => self.launch(ctx, id, None, StartMode::Resume),
            Some(Action::Confirm(confirm)) => {
                self.confirm = Some(confirm);
                Ok(())
            }
            Some(Action::Open(path)) => open_path(&path),
            None => Ok(()),
        };
        if let Err(error) = result {
            self.notice = error_text(&error);
        }
    }

    pub(super) fn confirm_ui(&mut self, ctx: &Context) {
        let Some(choice) = self.confirm else {
            return;
        };
        let (id, cancel) = match choice {
            Confirm::Cancel(id) => (id, true),
            Confirm::Restart(id) => (id, false),
        };
        let Some(entry) = self.library.entries.iter().find(|entry| entry.id == id) else {
            self.confirm = None;
            return;
        };
        if entry.state == State::Complete || entry.canceled {
            self.confirm = None;
            return;
        }
        let modal = egui::Modal::new(egui::Id::new("confirm-download-operation"))
            .frame(glass::card().fill(glass::popup())).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.label(RichText::new(text(if cancel { "Cancel download?" } else { "Restart download?" })).size(20.0).strong());
                ui.add(egui::Label::new(filename(&entry.path)).truncate());
                if cancel {
                    ui.label(text("Parally will stop this download and delete its partial data. The history entry will remain."));
                } else {
                    if entry.restart_required { ui.label(text(&entry.detail)); }
                    ui.label(text("Parally will download this file from the beginning and replace its partial data."));
                }
                ui.add_space(7.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let accept = ui.add(glass::button(if cancel { "Cancel & delete partial data" } else { "Restart from beginning" }, true, 165.0)).clicked();
                    let keep = ui.add(glass::button("Keep download", false, 115.0)).clicked();
                    (accept, keep)
                }).inner
            });
        let (accept, keep) = modal.inner;
        if accept || keep || modal.should_close() {
            self.confirm = None;
        }
        if accept {
            let outcome = if cancel {
                self.cancel_download(id)
            } else {
                self.launch(ctx, id, None, StartMode::Restart)
            };
            if let Err(error) = outcome {
                self.notice = error_text(&error);
            }
        }
    }
}
