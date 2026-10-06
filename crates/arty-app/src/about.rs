//! Help menu: About ARTY, licences, log folder, problem reports, and the notice
//! after a crash (plans/bench/B028_preview_pack.md).

use std::path::PathBuf;

use egui::RichText;

use crate::logging;
use crate::machine::Machine;
use crate::text::{Key, t};

/// The release this build belongs to.
pub const RELEASE_NAME: &str = "ARTY Preview 0";

// TODO(owner): the Google Form testers send problem reports through. Create it, then
// replace this address; Help > Report a problem opens it (and the log folder).
pub const FEEDBACK_URL: &str = "https://forms.gle/REPLACE_ME";

/// The preview licence: Thai, a line with `---`, then English (the same file goes in the zip).
const PREVIEW_LICENCE: &str = include_str!("../../../scripts/package/LICENSE-PREVIEW.txt");

/// The generated list of every dependency's licence (scripts\notices.ps1).
const LICENCES: &str = include_str!("../../../THIRD_PARTY_NOTICES.txt");

/// `0.2.0 (abc12345)`; the hash is left out of a build made without git.
pub fn version_string(package: &str, git_hash: &str) -> String {
    match git_hash {
        "" | "unknown" => package.to_owned(),
        hash => format!("{package} ({hash})"),
    }
}

/// This build's version, as the About dialog shows it.
pub fn version() -> String {
    version_string(env!("CARGO_PKG_VERSION"), env!("ARTY_GIT_HASH"))
}

/// The version with the release channel, for logs and crash reports.
pub fn version_line() -> String {
    format!("{} - {RELEASE_NAME}", version())
}

/// Opens a folder or a web page with the shell's default handler.
#[cfg(windows)]
fn shell_open(target: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, target) = (wide("open".as_ref()), wide(target));
    // SAFETY: both strings are NUL-terminated and outlive the call; the other pointers are null.
    let result = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), target.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    // Values above 32 are success.
    result as usize > 32
}

#[cfg(not(windows))]
fn shell_open(_target: &std::ffi::OsStr) -> bool {
    false
}

pub fn open_log_folder() {
    let dir = logging::dir();
    let _ = std::fs::create_dir_all(&dir);
    if !shell_open(dir.as_os_str()) {
        log::warn!("could not open {}", dir.display());
    }
}

pub fn open_feedback() {
    // The form asks for the newest log or crash file: show where it is.
    open_log_folder();
    if !shell_open(FEEDBACK_URL.as_ref()) {
        log::warn!("could not open {FEEDBACK_URL}");
    }
}

/// The licence text in the language of the UI, each paragraph on one line (the dialog wraps it).
fn preview_licence(lang: crate::text::Lang) -> String {
    let text = PREVIEW_LICENCE.replace("\r\n", "\n");
    let (th, en) = text.trim().split_once("---").unwrap_or((&text, &text));
    let part = if lang == crate::text::Lang::Th { th } else { en };
    let paragraphs: Vec<String> = part.trim().split("\n\n").map(|p| p.lines().map(str::trim).collect::<Vec<_>>().join(" ")).collect();
    paragraphs.join("\n\n")
}

/// The Help menu's items.
pub fn help_menu(ui: &mut egui::Ui, about: &mut About) {
    if ui.button(t(Key::HelpAbout)).clicked() {
        about.open = true;
        ui.close();
    }
    ui.separator();
    if ui.button(t(Key::HelpOpenLogs)).clicked() {
        open_log_folder();
        ui.close();
    }
    if ui.button(t(Key::HelpReport)).clicked() {
        open_feedback();
        ui.close();
    }
}

pub struct About {
    open: bool,
    /// The licence text split into lines, while it is shown.
    licences: Option<Vec<&'static str>>,
    /// The crash file the last run left, until the notice is closed.
    crash: Option<PathBuf>,
    /// `ARTY_DEBUG_PANIC=1`: panic a few frames after start-up, to try the crash report.
    debug_panic: bool,
    frames: u32,
}

/// The frame `ARTY_DEBUG_PANIC` panics in: after the first frame has been
/// presented (a crash before that marks the GPU backend as bad, gpu_setup.rs).
const DEBUG_PANIC_FRAME: u32 = 4;

impl About {
    pub fn new(open_at_start: bool) -> Self {
        Self {
            open: open_at_start,
            licences: None,
            crash: logging::previous_crash().map(PathBuf::from),
            debug_panic: std::env::var_os("ARTY_DEBUG_PANIC").is_some(),
            frames: 0,
        }
    }

    /// A dialog is up: the keyboard belongs to it.
    pub fn is_open(&self) -> bool {
        self.open || self.crash.is_some()
    }

    pub fn ui(&mut self, ctx: &egui::Context, machine: &Machine) {
        self.frames += 1;
        if self.debug_panic && self.frames == DEBUG_PANIC_FRAME {
            panic!("ARTY_DEBUG_PANIC is set");
        }
        if self.open {
            self.about_dialog(ctx, machine);
        }
        if self.crash.is_some() {
            self.crash_notice(ctx);
        }
    }

    fn about_dialog(&mut self, ctx: &egui::Context, machine: &Machine) {
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("about")).show(ctx, |ui| {
            if let Some(lines) = &self.licences {
                ui.set_width(640.0);
                ui.heading(t(Key::AboutLicences));
                ui.add_space(6.0);
                let row = ui.text_style_height(&egui::TextStyle::Monospace);
                egui::ScrollArea::both().auto_shrink(false).max_height(360.0).show_rows(ui, row, lines.len(), |ui, range| {
                    for line in &lines[range] {
                        ui.add(egui::Label::new(RichText::new(*line).monospace()).wrap_mode(egui::TextWrapMode::Extend));
                    }
                });
                ui.add_space(8.0);
                return ui.button(t(Key::AboutBack)).clicked().then_some(false);
            }
            ui.set_width(380.0);
            ui.heading(RELEASE_NAME);
            ui.label(t(Key::AboutVersion).replace("{}", &version()));
            ui.add_space(4.0);
            ui.add(egui::Label::new(t(Key::AboutPreviewNote)).wrap());
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(machine.log_line()).weak().small()).wrap());
            ui.add_space(6.0);
            egui::ScrollArea::vertical().id_salt("preview-licence").max_height(150.0).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(preview_licence(crate::text::current_lang())).small()).wrap());
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(t(Key::AboutLicences)).clicked() {
                    return Some(true);
                }
                ui.button(t(Key::AboutClose)).clicked().then_some(false)
            })
            .inner
        });
        match modal.inner {
            Some(true) => self.licences = Some(LICENCES.lines().collect()),
            Some(false) => {
                if self.licences.take().is_none() {
                    close = true;
                }
            }
            None => {
                if modal.should_close() {
                    close = self.licences.take().is_none();
                }
            }
        }
        if close {
            self.open = false;
        }
    }

    fn crash_notice(&mut self, ctx: &egui::Context) {
        let Some(file) = self.crash.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()) else {
            return;
        };
        let modal = egui::Modal::new(egui::Id::new("crash-notice")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.add(egui::Label::new(RichText::new(t(Key::CrashNotice)).strong()).wrap());
            ui.add_space(4.0);
            ui.weak(file);
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(t(Key::HelpOpenLogs)).clicked() {
                    open_log_folder();
                }
                ui.button(t(Key::AboutClose)).clicked()
            })
            .inner
        });
        if modal.inner || modal.should_close() {
            self.crash = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_strings() {
        assert_eq!(version_string("0.2.0", "abc12345"), "0.2.0 (abc12345)");
        assert_eq!(version_string("0.2.0", "unknown"), "0.2.0");
        assert_eq!(version_string("0.2.0", ""), "0.2.0");
    }

    #[test]
    fn this_build_names_its_version() {
        let v = version();
        assert!(v.starts_with(env!("CARGO_PKG_VERSION")), "{v}");
        let hash = env!("ARTY_GIT_HASH");
        assert!(hash == "unknown" || hash.len() == 8 && hash.bytes().all(|b| b.is_ascii_hexdigit()), "{hash}");
        assert!(version_line().ends_with("ARTY Preview 0"));
    }

    #[test]
    fn the_preview_licence_has_both_languages() {
        use crate::text::Lang;
        let (th, en) = (preview_licence(Lang::Th), preview_licence(Lang::En));
        assert!(th.contains("ห้ามแจกจ่าย") && !th.contains("Do not redistribute"), "{th}");
        assert!(en.contains("Do not redistribute") && !en.contains("ห้ามแจกจ่าย"), "{en}");
        assert!(en.contains("All rights are reserved"));
    }

    #[test]
    fn the_licence_text_is_generated_and_complete() {
        for needle in ["MIT License", "Apache License", "SIL OPEN FONT LICENSE", "hokusai", "Noto Sans Thai", "egui-phosphor"] {
            assert!(LICENCES.contains(needle), "THIRD_PARTY_NOTICES.txt lacks {needle:?}; run scripts\\notices.ps1");
        }
        // Rows are drawn one per line without wrapping: keep lines short.
        assert!(LICENCES.lines().all(|l| l.len() <= 200), "a very long line");
    }
}
