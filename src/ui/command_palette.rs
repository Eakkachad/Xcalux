use crate::app::PaintApp;
use crate::commands::CommandId;

fn get_command_list() -> Vec<(CommandId, &'static str, &'static str)> {
    vec![
        // File
        (CommandId::NewDocument, "File: New Document", "Ctrl+N"),
        (CommandId::Open, "File: Open File", "Ctrl+O"),
        (CommandId::Save, "File: Save", "Ctrl+S"),
        (CommandId::SaveAs, "File: Save As", "Ctrl+Shift+S"),
        (CommandId::ExportPng, "File: Export as PNG", ""),
        (CommandId::ExportJpeg, "File: Export as JPEG", ""),
        (CommandId::ExportOra, "File: Export as OpenRaster (ORA)", ""),
        (CommandId::ImportImageAsLayer, "File: Import Image as Layer", ""),
        (CommandId::Preferences, "File: Preferences", ""),
        (CommandId::Exit, "File: Exit Application", "Ctrl+Q"),

        // Edit
        (CommandId::Undo, "Edit: Undo", "Ctrl+Z"),
        (CommandId::Redo, "Edit: Redo", "Ctrl+Y / Ctrl+Shift+Z"),
        (CommandId::Cut, "Edit: Cut", "Ctrl+X"),
        (CommandId::Copy, "Edit: Copy", "Ctrl+C"),
        (CommandId::CopyMerged, "Edit: Copy Merged", "Ctrl+Shift+C"),
        (CommandId::Paste, "Edit: Paste", "Ctrl+V"),
        (CommandId::PasteAsNewLayer, "Edit: Paste as New Layer", "Ctrl+Shift+V"),
        (CommandId::Clear, "Edit: Clear Selection/Layer", "Delete"),
        (CommandId::Fill, "Edit: Fill Selection/Layer", "Alt+Backspace"),

        // Canvas
        (CommandId::ResetRotation, "Canvas: Reset View Rotation", "Num0"),
        (CommandId::FlipViewHorizontal, "Canvas: Flip View Horizontally", "H"),
        (CommandId::FitToScreen, "Canvas: Fit Canvas to Screen", "Ctrl+Num0"),
        (CommandId::ActualSize, "Canvas: Show Actual Size (100%)", "Ctrl+Num1"),

        // Layer
        (CommandId::NewRasterLayer, "Layer: New Raster Layer", "Ctrl+Shift+N"),
        (CommandId::NewFolder, "Layer: New Folder", ""),
        (CommandId::NewVectorLayer, "Layer: New Vector Layer", ""),
        (CommandId::DuplicateLayer, "Layer: Duplicate Layer", "Ctrl+J"),
        (CommandId::DeleteLayer, "Layer: Delete Active Layer", "Ctrl+Delete"),
        (CommandId::MergeDown, "Layer: Merge Down", "Ctrl+E"),
        (CommandId::MergeVisible, "Layer: Merge Visible Layers", "Ctrl+Shift+E"),
        (CommandId::FlattenImage, "Layer: Flatten Image", ""),
        (CommandId::ClearLayer, "Layer: Clear Active Layer Content", ""),
        (CommandId::FillLayer, "Layer: Fill Active Layer with FG Color", ""),
        (CommandId::AddLayerMask, "Layer: Add Layer Mask", ""),
        (CommandId::ApplyLayerMask, "Layer: Apply Layer Mask", ""),
        (CommandId::DeleteLayerMask, "Layer: Delete Layer Mask", ""),
        (CommandId::ToggleLayerMask, "Layer: Toggle Layer Mask On/Off", ""),
        (CommandId::InvertLayerMask, "Layer: Invert Layer Mask", ""),

        // Selection
        (CommandId::SelectAll, "Selection: Select All", "Ctrl+A"),
        (CommandId::Deselect, "Selection: Deselect", "Ctrl+D"),
        (CommandId::InvertSelection, "Selection: Invert Selection", "Ctrl+I"),
        (CommandId::SelectionGrow, "Selection: Grow Selection Area", ""),
        (CommandId::SelectionShrink, "Selection: Shrink Selection Area", ""),
        (CommandId::SelectionFeather, "Selection: Feather Selection Border", ""),
        (CommandId::SelectionSmooth, "Selection: Smooth Selection Contour", ""),
        (CommandId::SelectionBorder, "Selection: Convert Selection to Border Outline", ""),

        // Tools
        (CommandId::ToolBrush, "Tool: Select Brush (🎨)", "B"),
        (CommandId::ToolEraser, "Tool: Select Eraser (🗑)", "E"),
        (CommandId::ToolFill, "Tool: Select Fill Bucket", "G"),
        (CommandId::ToolGradient, "Tool: Select Gradient Tool", "Shift+G"),
        (CommandId::ToolRectSelect, "Tool: Select Rectangle/Ellipse Selection", "M"),
        (CommandId::ToolLasso, "Tool: Select Free/Polygon Lasso Selection", "L"),
        (CommandId::ToolMagicWand, "Tool: Select Magic Wand Selection", "W"),
        (CommandId::ToolMove, "Tool: Select Move Layer Tool", "V"),
        (CommandId::ToolTransform, "Tool: Select Transform Layer Tool", "Ctrl+T"),
        (CommandId::ToolColorPicker, "Tool: Select Color Picker (Eyedropper)", "I"),
        (CommandId::ToolHand, "Tool: Select Hand Pan Tool", "Space"),
        (CommandId::ToolZoom, "Tool: Select Zoom Canvas Tool", ""),

        // Adjustments & Filters
        (CommandId::AdjustBrightnessContrast, "Filter: Brightness / Contrast Adjustment", ""),
        (CommandId::AdjustHueSaturation, "Filter: Hue / Saturation / Lightness Adjustment", ""),
        (CommandId::FilterGaussianBlur, "Filter: Gaussian Blur Filter", ""),

        // View & Interface
        (CommandId::ShowNavigator, "UI: Toggle Navigator Panel", ""),
        (CommandId::ShowColorPanel, "UI: Toggle Color Panel", ""),
        (CommandId::ShowLayers, "UI: Toggle Layers Manager", ""),
        (CommandId::ShowBrushPresets, "UI: Toggle Brush Workspace", ""),
        (CommandId::ShowStatusBar, "UI: Toggle Status Bar", ""),
        (CommandId::Fullscreen, "UI: Toggle Fullscreen Mode", "F11"),
        (CommandId::MinimalUi, "UI: Toggle Minimal UI (Hide Panels)", "Tab"),
        (CommandId::ResetWorkspace, "UI: Reset Panels Layout", ""),

        // Help
        (CommandId::KeyboardShortcuts, "Help: Edit Keyboard Shortcuts", ""),
        (CommandId::TabletDiagnostics, "Help: Show Tablet Diagnostics Screen", ""),
        (CommandId::PerformanceHud, "Help: Toggle Performance HUD Overlays", ""),
        (CommandId::About, "Help: About ARTY Paint App", ""),
    ]
}

pub fn draw_command_palette(app: &mut PaintApp, ctx: &egui::Context) {
    if !app.command_palette.open {
        return;
    }

    let screen_rect = ctx.input(|i| i.screen_rect());
    let width = 450.0;
    let height = 300.0;
    
    // Position at top-center of the screen
    let window_pos = egui::pos2(
        screen_rect.center().x - width * 0.5,
        screen_rect.top() + 60.0,
    );

    let mut execute_cmd: Option<CommandId> = None;
    let mut close_palette = false;

    egui::Window::new("Command Palette")
        .fixed_pos(window_pos)
        .fixed_size([width, height])
        .title_bar(false)
        .resizable(false)
        .frame(egui::Frame::window(&ctx.style())
            .fill(egui::Color32::from_rgb(30, 30, 30))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 80, 80)))
            .inner_margin(8.0)
            .outer_margin(0.0)
        )
        .show(ctx, |ui| {
            // Text field input
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("🔍").size(14.0).color(egui::Color32::from_gray(180)));
                let text_edit = egui::TextEdit::singleline(&mut app.command_palette.query)
                    .hint_text("Search command or tool...")
                    .desired_width(width - 36.0)
                    .frame(false)
                    .text_color(egui::Color32::WHITE);
                
                let res = ui.add(text_edit);
                res.request_focus();
                
                // If they press Escape, close
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close_palette = true;
                }
            });
            
            ui.separator();
            ui.add_space(4.0);

            // Filter commands
            let all_commands = get_command_list();
            let query = app.command_palette.query.trim().to_lowercase();
            let filtered: Vec<(CommandId, &str, &str)> = all_commands
                .into_iter()
                .filter(|(_, name, _)| query.is_empty() || name.to_lowercase().contains(&query))
                .collect();

            if filtered.is_empty() {
                ui.colored_label(egui::Color32::from_gray(140), "No matching commands found.");
                return;
            }

            // Keyboard navigation in list
            let count = filtered.len();
            let mut selected = app.command_palette.selected_index;
            if selected >= count {
                selected = 0;
            }

            if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                selected = (selected + 1) % count;
            }
            if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                selected = (selected + count - 1) % count;
            }
            app.command_palette.selected_index = selected;

            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if selected < count {
                    execute_cmd = Some(filtered[selected].0);
                    close_palette = true;
                }
            }

            // Render list
            egui::ScrollArea::vertical()
                .max_height(height - 60.0)
                .show(ui, |ui| {
                    for (idx, &(cmd, label, shortcut)) in filtered.iter().enumerate() {
                        let is_selected = idx == selected;
                        
                        // Allocate space for the row
                        let row_width = ui.available_width();
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::vec2(row_width, 24.0),
                            egui::Sense::click(),
                        );

                        // Draw selection highlight
                        let painter = ui.painter();
                        if is_selected {
                            painter.rect_filled(
                                rect,
                                3.0,
                                egui::Color32::from_rgb(0, 120, 215),
                            );
                        } else if resp.hovered() {
                            painter.rect_filled(
                                rect,
                                3.0,
                                egui::Color32::from_rgb(50, 50, 50),
                            );
                        }

                        // Label text
                        let text_color = if is_selected {
                            egui::Color32::WHITE
                        } else {
                            egui::Color32::from_gray(210)
                        };
                        painter.text(
                            egui::pos2(rect.left() + 6.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::proportional(11.0),
                            text_color,
                        );

                        // Shortcut text
                        if !shortcut.is_empty() {
                            let sc_color = if is_selected {
                                egui::Color32::from_rgb(220, 220, 220)
                            } else {
                                egui::Color32::from_gray(130)
                            };
                            painter.text(
                                egui::pos2(rect.right() - 6.0, rect.center().y),
                                egui::Align2::RIGHT_CENTER,
                                shortcut,
                                egui::FontId::proportional(10.0),
                                sc_color,
                            );
                        }

                        if resp.clicked() {
                            execute_cmd = Some(cmd);
                            close_palette = true;
                        }
                    }
                });
        });

    if close_palette {
        app.command_palette.open = false;
        ctx.memory_mut(|mem| mem.request_focus(egui::Id::new("canvas"))); // return focus to canvas
    }

    if let Some(cmd) = execute_cmd {
        app.command(cmd);
    }
}
