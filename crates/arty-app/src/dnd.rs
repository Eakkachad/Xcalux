//! Files dropped anywhere on the window open like File > Open does, with a
//! "Drop to open" cue while they are over it. Only `.arty` files open.

use std::path::{Path, PathBuf};

use egui::{Color32, CornerRadius, FontId, Stroke, StrokeKind};
use egui_phosphor::regular as icon;

use crate::shell::{FileRequest, Shell};
use crate::studio::Studio;
use crate::text::{Key, t};
use crate::theme::Palette;

/// Whether `path` is a file ARTY opens.
pub fn is_openable(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("arty"))
}

/// What a drop of `files` does.
#[derive(Debug, PartialEq, Eq)]
pub enum Drop {
    /// The first file ARTY opens.
    Open(PathBuf),
    /// Files, but none of them one ARTY opens.
    Ignored,
}

/// `None` when nothing was dropped.
pub fn pick(files: &[PathBuf]) -> Option<Drop> {
    if files.is_empty() {
        return None;
    }
    let first = files.iter().find(|p| is_openable(p));
    Some(first.map_or(Drop::Ignored, |p| Drop::Open(p.clone())))
}

/// Act on files dropped this frame; `blocked` while a dialog is open.
pub fn handle(ctx: &egui::Context, studio: &mut Studio, shell: &mut Shell, blocked: bool) {
    let dropped = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect::<Vec<_>>());
    match pick(&dropped) {
        Some(Drop::Open(path)) if !blocked && shell.file_request.is_none() => {
            shell.open_path = Some(path);
            shell.file_request = Some(FileRequest::OpenPath);
        }
        Some(Drop::Ignored) if !blocked => studio.notice = Some(t(Key::HomeDropIgnored).to_owned()),
        _ => {}
    }
}

/// A veil over the whole window while an openable file is dragged over it.
pub fn overlay(ctx: &egui::Context, pal: &Palette) {
    let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty() && i.raw.hovered_files.iter().any(|f| f.path.as_deref().is_none_or(is_openable)));
    if !hovering {
        return;
    }
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("drop-overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .interactable(false)
        .show(ctx, |ui| {
            let painter = ui.painter();
            painter.rect_filled(screen, CornerRadius::ZERO, pal.accent.gamma_multiply(0.16));
            painter.rect_stroke(screen.shrink(10.0), CornerRadius::same(12), Stroke::new(3.0, pal.accent), StrokeKind::Inside);
            let text = format!("{}  {}", icon::DOWNLOAD_SIMPLE, t(Key::HomeDropToOpen));
            let galley = painter.layout_no_wrap(text, FontId::proportional(28.0), pal.text);
            let pill = egui::Rect::from_center_size(screen.center(), galley.size() + egui::vec2(48.0, 28.0));
            painter.rect(pill, CornerRadius::same(10), pal.popup, Stroke::new(1.5, pal.accent), StrokeKind::Inside);
            painter.galley(pill.center() - galley.size() * 0.5, galley, Color32::PLACEHOLDER);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn only_arty_files_open() {
        assert!(is_openable(Path::new("C:\\a\\page.arty")));
        assert!(is_openable(Path::new("page.ARTY")));
        for no in ["page.png", "page.arty.png", "page", "arty", "C:\\a.arty\\x", "page.psd"] {
            assert!(!is_openable(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn a_drop_opens_the_first_openable_file() {
        assert_eq!(pick(&[]), None);
        assert_eq!(pick(&[p("a.arty")]), Some(Drop::Open("a.arty".into())));
        assert_eq!(pick(&[p("x.png"), p("b.ARTY"), p("c.arty")]), Some(Drop::Open("b.ARTY".into())));
        assert_eq!(pick(&[p("x.png"), p("y.psd")]), Some(Drop::Ignored));
    }

    #[derive(Debug)]
    struct Fake(PathBuf);

    impl egui::DroppedFile for Fake {
        fn path(&self) -> &Path {
            &self.0
        }

        fn bytes(&self) -> Result<Vec<u8>, String> {
            Err("test".into())
        }
    }

    /// The veil shows while an openable file hovers over the window, and not for other files.
    #[test]
    fn the_overlay_shows_for_openable_hovers_only() {
        use crate::theme::ThemeKind;
        let painted = |hovered: Vec<egui::HoveredFile>| {
            let ctx = egui::Context::default();
            let raw = egui::RawInput { hovered_files: hovered, ..Default::default() };
            let out = ctx.run_ui(raw, |ui| overlay(ui.ctx(), &ThemeKind::Light.palette()));
            let shapes = out.shapes.len();
            out.drop_without_applying_deltas();
            shapes
        };
        let hover = |name: &str| egui::HoveredFile { path: Some(name.into()), ..Default::default() };
        let idle = painted(vec![]);
        assert_eq!(painted(vec![hover("x.png")]), idle, "other files get no veil");
        assert!(painted(vec![hover("x.arty")]) > idle);
        assert!(painted(vec![egui::HoveredFile::default()]) > idle, "a hover without a path might be one");
    }

    #[test]
    fn a_dropped_file_asks_for_an_open_and_others_a_toast() {
        use crate::theme::ThemeKind;
        let drop_of = |files: Vec<egui::DroppedFileHandle>| {
            let ctx = egui::Context::default();
            let mut studio = Studio::new(arty_core::Document::new(64, 64, 72));
            let mut shell = Shell::new(ThemeKind::Dark);
            let raw = egui::RawInput { dropped_files: files, ..Default::default() };
            ctx.run_ui(raw, |ui| handle(ui.ctx(), &mut studio, &mut shell, false)).drop_without_applying_deltas();
            (shell.file_request, shell.open_path, studio.notice)
        };
        let file = |name: &str| -> egui::DroppedFileHandle { std::sync::Arc::new(Fake(PathBuf::from(name))) };
        assert_eq!(drop_of(vec![file("C:\\a\\page.arty")]), (Some(FileRequest::OpenPath), Some("C:\\a\\page.arty".into()), None));
        let (req, path, notice) = drop_of(vec![file("C:\\a\\page.png")]);
        assert_eq!((req, path), (None, None));
        assert!(notice.is_some());
        assert_eq!(drop_of(vec![]), (None, None, None));
    }
}
