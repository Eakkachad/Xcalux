//! Window frame: menu, tool bar, dock area, status bar and dialogs.

use std::sync::mpsc::Receiver;

use arty_brush::BrushPreset;
use arty_core::Document;
use arty_io::{IoConfig, IoService, RecoveryDir};
use egui::{Color32, RichText};
use egui_dock::{DockArea, DockState};
use egui_phosphor::regular as icon;
use serde::{Deserialize, Serialize};

use crate::canvas::CanvasPane;
use crate::commands::{self, Command};
use crate::export;
use crate::files::{self, AutosaveSettings, FileController, NativeDialogs};
use crate::panels::{self, PreviewCache, Tab, Viewer};
use crate::shell::{PAGE_PRESETS, Shell};
use crate::studio::{InputSettings, Rgb, Studio};
use crate::theme::{self, ThemeKind};

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
}

pub struct ArtyApp {
    studio: Studio,
    shell: Shell,
    canvas: CanvasPane,
    previews: PreviewCache,
    dock: DockState<Tab>,
    export_job: Option<Receiver<String>>,
    files: FileController,
}

impl ArtyApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        // Ctrl+= / Ctrl+- / Ctrl+0 zoom the canvas (commands.rs), never the UI.
        // The option isn't persisted, but a zoom factor saved by an older build is.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        cc.egui_ctx.set_zoom_factor(1.0);
        let saved: Option<Persisted> = cc.storage.and_then(|s| eframe::get_value(s, STORAGE_KEY));

        let (_, w, h, dpi) = PAGE_PRESETS[3];
        let mut studio = Studio::new(Document::new(w, h, dpi));
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
        }
        if std::env::var_os("ARTY_DEMO").is_some() {
            crate::demo::paint_sample_strokes(&mut studio);
        }

        let ctx = cc.egui_ctx.clone();
        let io = IoService::spawn(IoConfig::new(RecoveryDir::default_path()), move || ctx.request_repaint());
        let files = FileController::new(io, Box::new(NativeDialogs), &studio);
        let mut shell = Shell::new(theme_kind);
        shell.autosave = autosave;

        Self {
            studio,
            shell,
            canvas: CanvasPane::new(cc.wgpu_render_state.clone()),
            previews: PreviewCache::default(),
            dock,
            export_job: None,
            files,
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            let studio = &mut self.studio;
            let shell = &mut self.shell;
            let item = |ui: &mut egui::Ui, cmd: Command, studio: &mut Studio, shell: &mut Shell| {
                let mut b = egui::Button::new(cmd.label());
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
                for cmd in [Command::Undo, Command::Redo, Command::ClearLayer] {
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
                ] {
                    item(ui, cmd, studio, shell);
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
            egui::ComboBox::from_id_salt("page-preset").width(340.0).selected_text(PAGE_PRESETS[form.preset].0).show_ui(ui, |ui| {
                for (i, (name, w, h, dpi)) in PAGE_PRESETS.iter().enumerate() {
                    if ui.selectable_label(form.preset == i, *name).clicked() {
                        form.preset = i;
                        form.width = *w;
                        form.height = *h;
                        form.dpi = *dpi;
                    }
                }
            });
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
            self.shell.new_doc_open = false;
        } else if cancel || modal.should_close() {
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
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
        if self.shell.export_requested {
            self.shell.export_requested = false;
            self.export_job = export::export_png(&self.studio.doc);
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
        };
        egui::CentralPanel::no_frame().show(ui, |ui| {
            DockArea::new(&mut self.dock)
                .style(style)
                .show_leaf_collapse_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_inside(ui, &mut viewer);
        });

        self.new_document_dialog(&ctx);
        self.files.ui(&ctx, &mut self.studio, &mut self.shell);
        self.toasts(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let p = Persisted {
            theme: self.shell.theme,
            layout_version: LAYOUT_VERSION,
            dock: self.dock.clone(),
            presets: self.studio.presets.clone(),
            input: self.studio.input,
            swatches: self.studio.color.swatches.clone(),
            autosave: self.shell.autosave,
        };
        eframe::set_value(storage, STORAGE_KEY, &p);
    }
}
