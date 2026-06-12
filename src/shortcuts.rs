use crate::commands::CommandId;
use egui::Key;

#[derive(Debug, Clone)]
pub struct KeyBinding {
    pub key: Key,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl KeyBinding {
    pub fn matches(&self, key: Key, ctrl: bool, shift: bool, alt: bool) -> bool {
        self.key == key && self.ctrl == ctrl && self.shift == shift && self.alt == alt
    }

    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        s.push_str(&format!("{:?}", self.key));
        s
    }

    pub fn from_event(key: Key, ctrl: bool, shift: bool, alt: bool) -> Self {
        Self {
            key,
            ctrl,
            shift,
            alt,
        }
    }
}

#[allow(dead_code)]
pub struct ShortcutEntry {
    pub command: CommandId,
    pub primary: Option<KeyBinding>,
    pub secondary: Option<KeyBinding>,
    pub name: &'static str,
    pub category: &'static str,
}

pub fn default_shortcuts() -> Vec<ShortcutEntry> {
    vec![
        // File
        ShortcutEntry {
            command: CommandId::NewDocument,
            primary: Some(KeyBinding {
                key: Key::N,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "New Document",
            category: "File",
        },
        ShortcutEntry {
            command: CommandId::Open,
            primary: Some(KeyBinding {
                key: Key::O,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Open",
            category: "File",
        },
        ShortcutEntry {
            command: CommandId::Save,
            primary: Some(KeyBinding {
                key: Key::S,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Save",
            category: "File",
        },
        ShortcutEntry {
            command: CommandId::SaveAs,
            primary: Some(KeyBinding {
                key: Key::S,
                ctrl: true,
                shift: true,
                alt: false,
            }),
            secondary: None,
            name: "Save As",
            category: "File",
        },
        ShortcutEntry {
            command: CommandId::Exit,
            primary: Some(KeyBinding {
                key: Key::Q,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Exit",
            category: "File",
        },
        // Edit
        ShortcutEntry {
            command: CommandId::Undo,
            primary: Some(KeyBinding {
                key: Key::Z,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Undo",
            category: "Edit",
        },
        ShortcutEntry {
            command: CommandId::Redo,
            primary: Some(KeyBinding {
                key: Key::Y,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: Some(KeyBinding {
                key: Key::Z,
                ctrl: true,
                shift: true,
                alt: false,
            }),
            name: "Redo",
            category: "Edit",
        },
        ShortcutEntry {
            command: CommandId::Cut,
            primary: Some(KeyBinding {
                key: Key::X,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Cut",
            category: "Edit",
        },
        ShortcutEntry {
            command: CommandId::Copy,
            primary: Some(KeyBinding {
                key: Key::C,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Copy",
            category: "Edit",
        },
        ShortcutEntry {
            command: CommandId::CopyMerged,
            primary: Some(KeyBinding {
                key: Key::C,
                ctrl: true,
                shift: true,
                alt: false,
            }),
            secondary: None,
            name: "Copy Merged",
            category: "Edit",
        },
        ShortcutEntry {
            command: CommandId::Paste,
            primary: Some(KeyBinding {
                key: Key::V,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Paste",
            category: "Edit",
        },
        ShortcutEntry {
            command: CommandId::PasteAsNewLayer,
            primary: Some(KeyBinding {
                key: Key::V,
                ctrl: true,
                shift: true,
                alt: false,
            }),
            secondary: None,
            name: "Paste as New Layer",
            category: "Edit",
        },
        ShortcutEntry {
            command: CommandId::Clear,
            primary: Some(KeyBinding {
                key: Key::Delete,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Clear",
            category: "Edit",
        },
        ShortcutEntry {
            command: CommandId::Fill,
            primary: Some(KeyBinding {
                key: Key::Backspace,
                ctrl: false,
                shift: false,
                alt: true,
            }),
            secondary: None,
            name: "Fill",
            category: "Edit",
        },
        // Canvas
        ShortcutEntry {
            command: CommandId::FitToScreen,
            primary: Some(KeyBinding {
                key: Key::Num0,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Fit to Screen",
            category: "Canvas",
        },
        ShortcutEntry {
            command: CommandId::ActualSize,
            primary: Some(KeyBinding {
                key: Key::Num1,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Actual Size",
            category: "Canvas",
        },
        ShortcutEntry {
            command: CommandId::FlipViewHorizontal,
            primary: Some(KeyBinding {
                key: Key::H,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Flip View Horizontal",
            category: "Canvas",
        },
        ShortcutEntry {
            command: CommandId::ResetRotation,
            primary: Some(KeyBinding {
                key: Key::Num0,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Reset Rotation",
            category: "Canvas",
        },
        // Layer
        ShortcutEntry {
            command: CommandId::NewRasterLayer,
            primary: Some(KeyBinding {
                key: Key::N,
                ctrl: true,
                shift: true,
                alt: false,
            }),
            secondary: None,
            name: "New Raster Layer",
            category: "Layer",
        },
        ShortcutEntry {
            command: CommandId::DuplicateLayer,
            primary: Some(KeyBinding {
                key: Key::J,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Duplicate Layer",
            category: "Layer",
        },
        ShortcutEntry {
            command: CommandId::MergeDown,
            primary: Some(KeyBinding {
                key: Key::E,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Merge Down",
            category: "Layer",
        },
        ShortcutEntry {
            command: CommandId::MergeVisible,
            primary: Some(KeyBinding {
                key: Key::E,
                ctrl: true,
                shift: true,
                alt: false,
            }),
            secondary: None,
            name: "Merge Visible",
            category: "Layer",
        },
        ShortcutEntry {
            command: CommandId::DeleteLayer,
            primary: Some(KeyBinding {
                key: Key::Delete,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Delete Layer",
            category: "Layer",
        },
        // Selection
        ShortcutEntry {
            command: CommandId::SelectAll,
            primary: Some(KeyBinding {
                key: Key::A,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Select All",
            category: "Selection",
        },
        ShortcutEntry {
            command: CommandId::Deselect,
            primary: Some(KeyBinding {
                key: Key::D,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Deselect",
            category: "Selection",
        },
        ShortcutEntry {
            command: CommandId::Reselect,
            primary: Some(KeyBinding {
                key: Key::D,
                ctrl: true,
                shift: true,
                alt: false,
            }),
            secondary: None,
            name: "Reselect",
            category: "Selection",
        },
        ShortcutEntry {
            command: CommandId::InvertSelection,
            primary: Some(KeyBinding {
                key: Key::I,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Invert Selection",
            category: "Selection",
        },
        ShortcutEntry {
            command: CommandId::TransformSelection,
            primary: Some(KeyBinding {
                key: Key::T,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Transform",
            category: "Selection",
        },
        ShortcutEntry {
            command: CommandId::ToggleSelectionOverlay,
            primary: Some(KeyBinding {
                key: Key::H,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Toggle Selection Overlay",
            category: "Selection",
        },
        // Tools
        ShortcutEntry {
            command: CommandId::ToolBrush,
            primary: Some(KeyBinding {
                key: Key::B,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Brush Tool",
            category: "Tools",
        },
        ShortcutEntry {
            command: CommandId::ToolEraser,
            primary: Some(KeyBinding {
                key: Key::E,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Eraser Tool",
            category: "Tools",
        },
        ShortcutEntry {
            command: CommandId::ToolFill,
            primary: Some(KeyBinding {
                key: Key::G,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Fill Tool",
            category: "Tools",
        },
        ShortcutEntry {
            command: CommandId::ToolGradient,
            primary: Some(KeyBinding {
                key: Key::G,
                ctrl: false,
                shift: true,
                alt: false,
            }),
            secondary: Some(KeyBinding {
                key: Key::G,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            name: "Gradient Tool",
            category: "Tools",
        },
        ShortcutEntry {
            command: CommandId::ToolRectSelect,
            primary: Some(KeyBinding {
                key: Key::M,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Rect Select",
            category: "Tools",
        },
        ShortcutEntry {
            command: CommandId::ToolLasso,
            primary: Some(KeyBinding {
                key: Key::L,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Lasso",
            category: "Tools",
        },
        ShortcutEntry {
            command: CommandId::ToolMagicWand,
            primary: Some(KeyBinding {
                key: Key::W,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Magic Wand",
            category: "Tools",
        },
        ShortcutEntry {
            command: CommandId::ToolMove,
            primary: Some(KeyBinding {
                key: Key::V,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Move Tool",
            category: "Tools",
        },
        ShortcutEntry {
            command: CommandId::ToolTransform,
            primary: Some(KeyBinding {
                key: Key::T,
                ctrl: true,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Transform",
            category: "Tools",
        },
        ShortcutEntry {
            command: CommandId::ToolColorPicker,
            primary: Some(KeyBinding {
                key: Key::I,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Color Picker",
            category: "Tools",
        },
        // View
        ShortcutEntry {
            command: CommandId::Fullscreen,
            primary: Some(KeyBinding {
                key: Key::F11,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Fullscreen",
            category: "View",
        },
        ShortcutEntry {
            command: CommandId::MinimalUi,
            primary: Some(KeyBinding {
                key: Key::Tab,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Minimal UI",
            category: "View",
        },
        ShortcutEntry {
            command: CommandId::ShowGrid,
            primary: Some(KeyBinding {
                key: Key::Backtick,
                ctrl: false,
                shift: false,
                alt: false,
            }),
            secondary: None,
            name: "Show Grid",
            category: "View",
        },
    ]
}

pub struct ShortcutManager {
    pub entries: Vec<ShortcutEntry>,
}

impl ShortcutManager {
    pub fn new() -> Self {
        Self {
            entries: default_shortcuts(),
        }
    }

    pub fn find_command(&self, key: Key, ctrl: bool, shift: bool, alt: bool) -> Option<CommandId> {
        for entry in &self.entries {
            if let Some(ref binding) = entry.primary {
                if binding.matches(key, ctrl, shift, alt) {
                    return Some(entry.command);
                }
            }
            if let Some(ref binding) = entry.secondary {
                if binding.matches(key, ctrl, shift, alt) {
                    return Some(entry.command);
                }
            }
        }
        None
    }
}
