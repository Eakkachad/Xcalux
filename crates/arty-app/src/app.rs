//! Window frame: menu, tool bar, dock area, status bar and dialogs.

use std::sync::mpsc::Receiver;

use arty_brush::BrushPreset;
use arty_core::Document;
use arty_io::{IoConfig, IoService, RecoveryDir};
use arty_pen::PenStats;
use egui::{Color32, RichText};
use egui_dock::{DockArea, DockState};
use egui_phosphor::regular as icon;
use serde::{Deserialize, Serialize};

use crate::bench::{self, Bench, BenchDialogs};
use crate::canvas::CanvasPane;
use crate::commands::{self, Command, SelModify};
use crate::export;
use crate::files::{self, AutosaveSettings, FileController, NativeDialogs};
use crate::panels::{self, PreviewCache, Tab, ThumbCache, Viewer};
use crate::shell::{self, FileRequest, PAGE_PRESETS, Shell, new_doc_text};
use crate::studio::{DisplaySync, InputSettings, Rgb, Studio};
use crate::theme::{self, ThemeKind};
use crate::tools::{self, ToolOptions};

const STORAGE_KEY: &str = "arty-v2";
/// Bump when the default dock layout changes so old layouts are replaced.
const LAYOUT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Persisted {
    theme: ThemeKind,
    layout_version: u32,
    dock: DockState<Tab>,
    presets: Vec<BrushPreset>,
    input: InputSettings,
    swatches: Vec<Rgb>,
    #[serde(default)]
    autosave: AutosaveSettings,
    #[serde(default)]
    tool_opts: ToolOptions,
}

pub struct ArtyApp {
    studio: Studio,
    shell: Shell,
    canvas: CanvasPane,
    previews: PreviewCache,
    thumbs: ThumbCache,
    dock: DockState<Tab>,
    export_job: Option<Receiver<String>>,
    files: FileController,
    /// `ARTY_BENCH_*` hooks (bench.rs); `None` in normal runs.
    bench: Option<Bench>,
    /// A bench variable is set: nothing is saved (bench.rs).
    bench_run: bool,
    /// Display sync the surface was started with. eframe 0.36 applies the
    /// setting only at start-up (main.rs), so this is what is running.
    running_sync: DisplaySync,
}

/// Status bar latency readout. `in→frame` is OS sample time to canvas processing only.
/// The frame figures belong to `running`; a different `selected` Display sync
/// is named as what the next start gives (main.rs starts Fast vsync as Low latency).
fn latency_text(st: PenStats, frame_ms: f32, running: DisplaySync, selected: DisplaySync) -> String {
    let pen = if st.native {
        format!("pen {:.0} Hz · in→frame {:.1} ms (max {:.1})", st.rate_hz, st.age_ms, st.age_max_ms)
    } else {
        "pen: system".to_owned()
    };
    let mut s = format!("{pen} · frame {frame_ms:.1} ms · {}", running.label());
    if selected != running {
        let next = DisplaySync::from_surface_config(selected.surface_config(false), false).unwrap_or(selected);
        if next == running {
            s.push_str(&format!(" ({} not applied)", selected.label()));
        } else {
            s.push_str(&format!(" ({} after restart)", next.label()));
        }
    }
    if st.dropped > 0 {
        s.push_str(&format!(" · dropped {}", st.dropped));
    }
    s
}

impl ArtyApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        // Ctrl+= / Ctrl+- / Ctrl+0 zoom the canvas (commands.rs), never the UI.
        // The option isn't persisted, but a zoom factor saved by an older build is.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        cc.egui_ctx.set_zoom_factor(bench::zoom().unwrap_or(1.0));
        let saved: Option<Persisted> = cc.storage.and_then(|s| eframe::get_value(s, STORAGE_KEY));

        let (w, h, dpi) = shell::DEFAULT_PAGE;
        let mut studio = Studio::new(Document::new(w, h, dpi));
        studio.history.set_budget(arty_core::undo_budget(arty_io::physical_memory()));
        studio.history.set_release(crate::studio::undo_release());
        log::info!("undo budget {} MiB", studio.history.budget() >> 20);
        // Mailbox panics where unsupported and eframe exposes no surface capabilities.
        studio.fast_vsync_ok =
            cc.wgpu_render_state.as_ref().is_some_and(|r| r.adapter.get_info().backend == egui_wgpu::wgpu::Backend::Dx12);
        let mut dock = panels::default_layout();
        let mut theme_kind = ThemeKind::Dark;
        let mut autosave = AutosaveSettings::default();
        if let Some(p) = saved {
            theme_kind = p.theme;
            autosave = p.autosave;
            if p.layout_version == LAYOUT_VERSION {
                dock = p.dock;
            }
            if !p.presets.is_empty() {
                studio.presets = p.presets;
                studio.select_tool(studio.tool);
            }
            studio.input = p.input;
            if !p.swatches.is_empty() {
                studio.color.swatches = p.swatches;
            }
            studio.opts = p.tool_opts;
        }
        if bench::active() {
            // Runs on different profiles do the same work (bench.rs).
            let InputSettings { native_pen, display_sync, .. } = studio.input;
            studio.input = InputSettings { native_pen, display_sync, ..InputSettings::default() };
            autosave = AutosaveSettings::default();
        }
        // What main.rs started the surface with (the render state is created from it).
        let started = cc.wgpu_render_state.as_ref().map_or(studio.input.display_sync.surface_config(false), |r| r.surface_config);
        let running_sync = DisplaySync::from_surface_config(started, studio.fast_vsync_ok).unwrap_or_default();
        if std::env::var_os("ARTY_DEMO").is_some() {
            crate::demo::paint_sample_strokes(&mut studio);
        }

        let ctx = cc.egui_ctx.clone();
        let mut io_config = IoConfig::new(RecoveryDir::default_path());
        if let Some(n) = bench::io_threads() {
            io_config.threads = n;
        }
        let io_threads = io_config.threads;
        let load_budget = io_config.load.limits.max_decoded_bytes;
        let io = IoService::spawn(io_config, move || ctx.request_repaint());
        let pen = arty_pen::install(cc);
        let bench = Bench::from_env(studio.doc_epoch, pen.as_ref());
        let mut shell = Shell::new(theme_kind);
        shell.autosave = autosave;
        let dialogs: Box<dyn files::FileDialogs> = match bench.as_ref().and_then(|b| b.open.as_ref()) {
            Some(_) => {
                shell.file_request = Some(FileRequest::Open);
                Box::new(BenchDialogs(bench::open_path()))
            }
            None => Box::new(NativeDialogs),
        };
        let files = FileController::new(io, dialogs, &studio);
        if bench::active() {
            let autosave_str = if autosave.enabled { format!("{} s", autosave.interval_secs) } else { "off".to_owned() };
            bench::report_threads(io_threads, load_budget, &autosave_str);
        }

        Self {
            studio,
            shell,
            canvas: CanvasPane::new(cc.wgpu_render_state.clone(), pen),
            previews: PreviewCache::default(),
            thumbs: ThumbCache::default(),
            dock,
            export_job: None,
            files,
            bench,
            bench_run: bench::active(),
            running_sync,
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            let studio = &mut self.studio;
            let shell = &mut self.shell;
            let item = |ui: &mut egui::Ui, cmd: Command, studio: &mut Studio, shell: &mut Shell| {
                let label = match cmd {
                    Command::ClearLayer if studio.doc.has_selection() => "Clear Selected Area",
                    _ => cmd.label(),
                };
                let mut b = egui::Button::new(label);
                if let Some(sc) = commands::shortcut_for(cmd) {
                    b = b.shortcut_text(ui.ctx().format_shortcut(&sc));
                }
                if ui.add(b).clicked() {
                    commands::execute(cmd, studio, shell);
                    ui.close();
                }
            };
            ui.menu_button("File", |ui| {
                for cmd in [Command::NewDocument, Command::Open, Command::Save, Command::SaveAs, Command::ExportPng] {
                    item(ui, cmd, studio, shell);
                }
                ui.separator();
                item(ui, Command::PageSetup, studio, shell);
                ui.separator();
                ui.menu_button("Autosave", |ui| {
                    if ui.selectable_label(shell.autosave.enabled, "Autosave enabled").clicked() {
                        commands::execute(Command::ToggleAutosave, studio, shell);
                    }
                    ui.horizontal(|ui| {
                        ui.label("Every");
                        let range = files::MIN_INTERVAL_SECS..=3600;
                        ui.add(egui::DragValue::new(&mut shell.autosave.interval_secs).range(range).suffix(" s"));
                    });
                });
                ui.separator();
                item(ui, Command::Quit, studio, shell);
            });
            ui.menu_button("Edit", |ui| {
                for cmd in [Command::Undo, Command::Redo, Command::ClearLayer, Command::FillSelection] {
                    item(ui, cmd, studio, shell);
                }
                ui.separator();
                for cmd in [
                    Command::Transform,
                    Command::CommitTransform,
                    Command::CancelTransform,
                    Command::FlipTransform { horizontal: true },
                    Command::FlipTransform { horizontal: false },
                    Command::RotateTransform90 { cw: true },
                    Command::RotateTransform90 { cw: false },
                ] {
                    item(ui, cmd, studio, shell);
                }
            });
            ui.menu_button("Layer", |ui| {
                for cmd in [
                    Command::NewLayer,
                    Command::NewFolder,
                    Command::DuplicateLayer,
                    Command::MergeDown,
                    Command::DeleteLayer,
                    Command::LayerUp,
                    Command::LayerDown,
                    Command::ToggleClip,
                    Command::ToggleLockAlpha,
                    Command::ToggleReferenceLayer,
                ] {
                    item(ui, cmd, studio, shell);
                }
                ui.separator();
                for cmd in [Command::NewFrameFolder, Command::DeletePanel] {
                    item(ui, cmd, studio, shell);
                }
            });
            ui.menu_button("Select", |ui| {
                for cmd in [Command::SelectAll, Command::Deselect, Command::InvertSelection] {
                    item(ui, cmd, studio, shell);
                }
                ui.separator();
                for m in [SelModify::Grow, SelModify::Shrink, SelModify::Feather] {
                    item(ui, Command::SelectionDialog(m), studio, shell);
                }
            });
            ui.menu_button("View", |ui| {
                for cmd in [
                    Command::ZoomIn,
                    Command::ZoomOut,
                    Command::ZoomFit,
                    Command::Zoom100,
                    Command::RotateLeft,
                    Command::RotateRight,
                    Command::RotateReset,
                    Command::FlipView,
                    Command::ToggleTheme,
                ] {
                    item(ui, cmd, studio, shell);
                }
                ui.separator();
                let page = &studio.opts.page;
                for (cmd, on) in [(Command::TogglePageGuides, page.show_guides), (Command::ToggleTrimShade, page.shade_outside_trim)] {
                    if ui.selectable_label(on, cmd.label()).clicked() {
                        commands::execute(cmd, studio, shell);
                        ui.close();
                    }
                }
            });
            ui.menu_button("Window", |ui| {
                for tab in Tab::PANELS {
                    let open = self.dock.find_tab(&tab).is_some();
                    if ui.selectable_label(open, tab.title()).clicked() {
                        match self.dock.find_tab(&tab) {
                            Some(path) => {
                                self.dock.remove_tab(path);
                            }
                            None => {
                                self.dock.add_window(vec![tab]);
                            }
                        }
                        ui.close();
                    }
                }
                ui.separator();
                item(ui, Command::ResetLayout, studio, shell);
            });

            // Quick undo/redo on the right, CSP command-bar style.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let theme_icon = if shell.theme == ThemeKind::Dark { icon::SUN } else { icon::MOON };
                if ui.button(theme_icon).on_hover_text("Toggle light/dark").clicked() {
                    shell.toggle_theme();
                }
                ui.separator();
                if ui.add_enabled(studio.history.can_redo(), egui::Button::new(icon::ARROW_U_UP_RIGHT)).on_hover_text("Redo").clicked() {
                    studio.redo();
                }
                if ui.add_enabled(studio.history.can_undo(), egui::Button::new(icon::ARROW_U_UP_LEFT)).on_hover_text("Undo").clicked() {
                    studio.undo();
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let s = &self.studio;
        let pal = self.shell.theme.palette();
        ui.horizontal(|ui| {
            let weak = |t: String| RichText::new(t).small().color(pal.text_weak);
            ui.label(RichText::new(s.tool.label()).small().strong());
            if let crate::studio::Tool::Brush(_) = s.tool {
                ui.label(weak(format!("{} · {:.1}px", s.preset().name, s.preset().size)));
            }
            ui.separator();
            ui.label(weak(format!(
                "{} × {} px · {} dpi",
                s.doc.width(),
                s.doc.height(),
                s.doc.dpi()
            )));
            ui.separator();
            ui.label(weak(format!("{:.1}%  {:.0}°{}", s.view.zoom * 100.0, s.view.rotation.to_degrees(), if s.view.flip_x { "  ⇋" } else { "" })));
            if let Some([x, y]) = self.shell.cursor_doc {
                ui.separator();
                ui.label(weak(format!("x {x:.0}  y {y:.0}")));
            }
            let file_status = self.files.status();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(st) = file_status {
                    ui.label(RichText::new(st).small().strong());
                    ui.separator();
                }
                let mb = s.doc.pixel_bytes() as f64 / (1024.0 * 1024.0);
                ui.label(weak(format!("{} layers · {mb:.1} MB", s.doc.layer_count())));
                ui.separator();
                if s.input.show_latency {
                    let frame_ms = ui.input(|i| i.stable_dt) * 1000.0;
                    let text = latency_text(self.canvas.pen_stats(), frame_ms, self.running_sync, s.input.display_sync);
                    ui.label(weak(text)).on_hover_text(
                        "in→frame: age of the newest pen sample when the canvas used it (OS timestamp to frame). \
                         It does not include rendering, presenting or the display.",
                    );
                    ui.separator();
                }
                let st = self.shell.last_sync;
                ui.label(weak(format!("composite {} tiles {:.1} ms", st.tiles, st.millis)));
            });
        });
    }

    fn new_document_dialog(&mut self, ctx: &egui::Context) {
        if !self.shell.new_doc_open {
            return;
        }
        let modal = egui::Modal::new(egui::Id::new("new-doc")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("New Page");
            ui.add_space(6.0);
            let form = &mut self.shell.new_doc;
            let current = shell::preset_for(form.width, form.height, form.dpi);
            let shown = current.map_or(new_doc_text::CUSTOM, |i| PAGE_PRESETS[i].0);
            egui::ComboBox::from_id_salt("page-preset").width(340.0).selected_text(shown).show_ui(ui, |ui| {
                for (i, (name, w, h, dpi)) in PAGE_PRESETS.iter().enumerate() {
                    if ui.selectable_label(current == Some(i), *name).clicked() {
                        form.width = *w;
                        form.height = *h;
                        form.dpi = *dpi;
                        // A plain preset has no page setup.
                        self.shell.new_doc_page = None;
                    }
                }
            });
            if let Some((w, h, dpi)) = tools::page::new_doc_ui(ui, &mut self.shell) {
                let form = &mut self.shell.new_doc;
                (form.width, form.height, form.dpi) = (w, h, dpi);
            }
            let form = &mut self.shell.new_doc;
            ui.add_space(6.0);
            egui::Grid::new("new-doc-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                let max = arty_render::gpu::MAX_PAGE_SIDE;
                ui.label("Width");
                ui.add(egui::DragValue::new(&mut form.width).range(64..=max).suffix(" px"));
                ui.end_row();
                ui.label("Height");
                ui.add(egui::DragValue::new(&mut form.height).range(64..=max).suffix(" px"));
                ui.end_row();
                ui.label("Resolution");
                ui.add(egui::DragValue::new(&mut form.dpi).range(72..=1200).suffix(" dpi"));
                ui.end_row();
                ui.label("");
                let mm = |px: u32| px as f32 / form.dpi as f32 * 25.4;
                ui.weak(format!("{:.0} × {:.0} mm", mm(form.width), mm(form.height)));
                ui.end_row();
                let m = shell::page_memory(form.width, form.height);
                ui.label("");
                ui.weak(format!("≈{} {} · {} {}", shell::mib(m.0), new_doc_text::PER_LAYER, shell::mib(m.1), new_doc_text::GPU));
                ui.end_row();
                if shell::page_memory_heavy(m, arty_io::physical_memory()) {
                    ui.label("");
                    ui.add(egui::Label::new(RichText::new(new_doc_text::HEAVY).color(ui.visuals().warn_fg_color)).wrap());
                    ui.end_row();
                }
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let create = ui.button(RichText::new("Create").strong()).clicked();
                let cancel = ui.button("Cancel").clicked();
                (create, cancel)
            })
            .inner
        });
        let (create, cancel) = modal.inner;
        if create {
            let f = &self.shell.new_doc;
            self.studio.new_document(f.width, f.height, f.dpi);
            // A manuscript preset's guides come with the page (not an edit).
            if let Some(page) = self.shell.new_doc_page.take() {
                self.studio.doc.set_page_unrecorded(Some(page));
            }
            self.shell.new_doc_open = false;
        } else if cancel || modal.should_close() {
            self.shell.new_doc_page = None;
            self.shell.new_doc_open = false;
        }
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if let Some(msg) = self.studio.notice.take() {
            self.shell.toast = Some((msg, now + 2.5));
        }
        if let Some(rx) = &self.export_job {
            if let Ok(msg) = rx.try_recv() {
                self.shell.toast = Some((msg, now + 4.0));
                self.export_job = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        if let Some((msg, until)) = &self.shell.toast {
            if now > *until {
                self.shell.toast = None;
            } else {
                egui::Area::new(egui::Id::new("toast"))
                    .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -40.0))
                    .order(egui::Order::Foreground)
                    .interactable(false)
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(RichText::new(msg.as_str()).color(Color32::from_rgb(240, 240, 240)));
                        });
                    });
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
    }
}

impl eframe::App for ArtyApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let frame_start = self.bench.is_some().then(std::time::Instant::now);
        // Applied on the next paint, only when the setting changed. eframe 0.36 does not
        // pass this back to its painter (see main.rs), so the start-up config is what counts.
        let want = self.studio.input.display_sync.surface_config(self.studio.fast_vsync_ok);
        if frame.wgpu_surface_config().is_some_and(|c| c != want) {
            frame.set_wgpu_surface_config(want);
        }
        if let Some(b) = &mut self.bench {
            b.frame(&ctx, &mut self.studio, self.running_sync);
        }
        if self.shell.theme_dirty {
            theme::apply(&ctx, self.shell.theme);
            self.shell.theme_dirty = false;
            self.previews.clear();
        }
        if self.shell.reset_layout_requested {
            self.dock = panels::default_layout();
            self.shell.reset_layout_requested = false;
        }
        // A modal dialog owns the keyboard: no document shortcuts behind it.
        if !self.canvas.is_busy() && !self.shell.new_doc_open && !self.files.has_modal() {
            commands::handle_shortcuts(&ctx, &mut self.studio, &mut self.shell);
        }
        self.files.tick(&ctx, &mut self.studio, &mut self.shell);
        self.thumbs.sync_doc(self.studio.doc_epoch);
        if self.shell.export_requested {
            self.shell.export_requested = false;
            self.studio.commit_transform();
            self.shell.export_dialog = true;
        }
        if let Some(crop) = tools::page::export_dialog(&ctx, &mut self.studio, &mut self.shell) {
            self.export_job = export::export_png(&self.studio.doc, crop);
        }
        if self.shell.quit_requested {
            // The file controller may cancel the close to ask about changes.
            self.shell.quit_requested = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("tools").resizable(false).exact_size(46.0).show(ui, |ui| {
            panels::toolbar::ui(ui, &mut self.studio, &mut self.shell);
        });

        let style = theme::dock_style(ui, self.shell.theme);
        let mut viewer = Viewer {
            studio: &mut self.studio,
            shell: &mut self.shell,
            canvas: &mut self.canvas,
            previews: &mut self.previews,
            thumbs: &mut self.thumbs,
        };
        egui::CentralPanel::no_frame().show(ui, |ui| {
            DockArea::new(&mut self.dock)
                .style(style)
                .show_leaf_collapse_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_inside(ui, &mut viewer);
        });

        self.new_document_dialog(&ctx);
        tools::select::dialogs(&ctx, &mut self.studio, &mut self.shell);
        tools::page::dialogs(&ctx, &mut self.studio, &mut self.shell);
        self.files.ui(&ctx, &mut self.studio, &mut self.shell);
        self.toasts(&ctx);
        if let (Some(b), Some(t)) = (&mut self.bench, frame_start) {
            b.frame_done(&ctx, t.elapsed(), &self.studio, &mut self.files, self.running_sync);
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(b) = &mut self.bench {
            b.raw_input(raw_input, ctx.pixels_per_point(), self.shell.canvas_center_px, &self.studio);
        }
    }

    fn persist_egui_memory(&self) -> bool {
        !self.bench_run
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.bench_run {
            return;
        }
        let p = Persisted {
            theme: self.shell.theme,
            layout_version: LAYOUT_VERSION,
            dock: self.dock.clone(),
            presets: self.studio.presets.clone(),
            input: self.studio.input,
            swatches: self.studio.color.swatches.clone(),
            autosave: self.shell.autosave,
            tool_opts: self.studio.opts.clone(),
        };
        eframe::set_value(storage, STORAGE_KEY, &p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::MemStorage;
    use arty_brush::pressure::PressureCurve;

    fn default_persisted() -> Persisted {
        Persisted {
            theme: ThemeKind::Light,
            layout_version: LAYOUT_VERSION,
            dock: panels::default_layout(),
            presets: arty_brush::default_presets(),
            input: InputSettings::default(),
            swatches: vec![[0.1, 0.2, 0.3]],
            autosave: AutosaveSettings::default(),
            tool_opts: ToolOptions::default(),
        }
    }

    /// Replaces the balanced `input:( ... )` group of a RON text.
    fn replace_input(ron: &str, legacy: &str) -> String {
        let start = ron.find("input:(").expect("input field") + "input:".len();
        let mut depth = 0;
        let end = ron[start..]
            .char_indices()
            .find_map(|(i, c)| {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then_some(start + i + 1)
            })
            .expect("balanced input group");
        format!("{}{legacy}{}", &ron[..start], &ron[end..])
    }

    #[test]
    fn legacy_persisted_blob_loads() {
        let mut st = MemStorage::default();
        eframe::set_value(&mut st, STORAGE_KEY, &default_persisted());
        let text = st.0[STORAGE_KEY].clone();
        assert!(text.contains("pressure_curve:(points:["), "{text}");
        let legacy = replace_input(&text, "(pressure_gamma:1.5,mouse_pressure:0.8)");
        assert!(legacy.contains("input:(pressure_gamma:1.5,mouse_pressure:0.8),"), "{legacy}");
        st.0.insert(STORAGE_KEY.to_owned(), legacy);

        let p: Persisted = eframe::get_value(&st, STORAGE_KEY).expect("legacy blob loads");
        assert_eq!(p.theme, ThemeKind::Light);
        assert_eq!(p.layout_version, LAYOUT_VERSION);
        assert!(Tab::PANELS.iter().all(|t| p.dock.find_tab(t).is_some()), "layout kept");
        assert_eq!(p.presets, arty_brush::default_presets());
        assert_eq!(p.swatches, vec![[0.1, 0.2, 0.3]]);
        assert_eq!(p.input.mouse_pressure, 0.8);
        assert_eq!(p.input.pressure_curve, PressureCurve::from_gamma(1.5));
        assert_eq!(p.input.display_sync, DisplaySync::LowLatency);
    }

    /// Tool options round-trip; a blob from before them loads with the defaults.
    #[test]
    fn tool_options_persist() {
        let mut st = MemStorage::default();
        let mut p = default_persisted();
        p.tool_opts.page.show_guides = false;
        eframe::set_value(&mut st, STORAGE_KEY, &p);
        let back: Persisted = eframe::get_value(&st, STORAGE_KEY).expect("loads");
        assert!(!back.tool_opts.page.show_guides);

        let text = st.0[STORAGE_KEY].clone();
        let start = text.find(",tool_opts:").expect("tool_opts written");
        let old = format!("{})", &text[..start]);
        st.0.insert(STORAGE_KEY.to_owned(), old);
        let back: Persisted = eframe::get_value(&st, STORAGE_KEY).expect("a blob without tool_opts loads");
        assert!(back.tool_opts.page.show_guides && !back.tool_opts.page.shade_outside_trim, "defaults");
        assert_eq!(back.theme, ThemeKind::Light);
    }

    #[test]
    fn latency_text_formats() {
        use DisplaySync::{FastVsync, LowLatency, Off, Smooth};
        let st = PenStats { native: true, rate_hz: 238.4, age_ms: 2.44, age_max_ms: 6.06, dropped: 0 };
        assert_eq!(
            latency_text(st, 6.94, LowLatency, LowLatency),
            "pen 238 Hz · in→frame 2.4 ms (max 6.1) · frame 6.9 ms · Low latency"
        );
        assert_eq!(
            latency_text(PenStats { dropped: 3, ..st }, 16.7, Smooth, Smooth),
            "pen 238 Hz · in→frame 2.4 ms (max 6.1) · frame 16.7 ms · Smooth · dropped 3"
        );
        assert_eq!(latency_text(PenStats::default(), 8.33, Off, Off), "pen: system · frame 8.3 ms · Off");
        // The figures are the running mode's; a new selection waits for a restart.
        assert_eq!(
            latency_text(PenStats::default(), 16.7, LowLatency, Off),
            "pen: system · frame 16.7 ms · Low latency (Off after restart)"
        );
        // Fast vsync always starts as Low latency.
        assert_eq!(
            latency_text(PenStats::default(), 16.7, LowLatency, FastVsync),
            "pen: system · frame 16.7 ms · Low latency (Fast vsync not applied)"
        );
        assert_eq!(
            latency_text(PenStats::default(), 16.7, Smooth, FastVsync),
            "pen: system · frame 16.7 ms · Smooth (Low latency after restart)"
        );
    }

    /// The overlay's running mode is read back from the start-up surface config.
    #[test]
    fn running_sync_from_start_up_config() {
        for sync in DisplaySync::ALL {
            // main.rs: the saved mode, never Mailbox.
            let running = DisplaySync::from_surface_config(sync.surface_config(false), true);
            let expect = if sync == DisplaySync::FastVsync { DisplaySync::LowLatency } else { sync };
            assert_eq!(running, Some(expect), "{sync:?}");
        }
        let mailbox = DisplaySync::FastVsync.surface_config(true);
        assert_eq!(DisplaySync::from_surface_config(mailbox, true), Some(DisplaySync::FastVsync));
        assert_eq!(DisplaySync::from_surface_config(mailbox, false), None);
    }
}
