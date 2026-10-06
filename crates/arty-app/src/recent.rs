//! Recent files: the most-recent-first list of `.arty` paths Home shows, and
//! the short-lived thread that reads their thumbnails (plans/ui_modes_plan.md
//! §4.1). Nothing here touches the UI thread's time: Home gets cards as they
//! arrive.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::SystemTime;

use crate::text::{Key, t};

/// Most paths kept.
pub const MAX_RECENT: usize = 12;

/// What identifies a path in the list: Windows paths ignore case and the kind
/// of slash, others compare exactly.
fn key(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) { s.replace('/', "\\").to_lowercase() } else { s.into_owned() }
}

/// The saved or opened `.arty` files, newest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recent {
    paths: Vec<PathBuf>,
}

impl Recent {
    /// A list of `paths` as given (newest first), without duplicates and
    /// within the cap.
    pub fn from_paths(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut list = Self::default();
        for p in paths {
            if list.paths.len() < MAX_RECENT && !list.paths.iter().any(|q| key(q) == key(&p)) {
                list.paths.push(p);
            }
        }
        list
    }

    /// The saved form (settings keep paths as text; one that is not valid text is left out).
    pub fn to_saved(&self) -> Vec<String> {
        self.paths.iter().filter_map(|p| p.to_str().map(str::to_owned)).collect()
    }

    pub fn from_saved(saved: &[String]) -> Self {
        Self::from_paths(saved.iter().map(PathBuf::from))
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// `path` was opened or saved: it moves to the front, the oldest falls off.
    pub fn touch(&mut self, path: &Path) {
        self.remove(path);
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(MAX_RECENT);
    }

    pub fn remove(&mut self, path: &Path) {
        let k = key(path);
        self.paths.retain(|p| key(p) != k);
    }
}

/// `ARTY_BENCH_RECENT`: paths separated by `|` (never in a Windows name) or `;` (the bench scripts split their variables on it).
pub fn parse_paths(s: &str) -> Option<Vec<PathBuf>> {
    let paths: Vec<PathBuf> = s.split(['|', ';']).map(str::trim).filter(|p| !p.is_empty()).map(PathBuf::from).collect();
    (!paths.is_empty()).then_some(paths)
}

/// "Edited 2 h ago" for a file last changed `age_secs` ago.
pub fn edited(age_secs: u64) -> String {
    let (key, n) = match age_secs {
        0..60 => return t(Key::HomeEditedNow).to_owned(),
        60..3600 => (Key::HomeEditedMin, age_secs / 60),
        3600..86_400 => (Key::HomeEditedHour, age_secs / 3600),
        86_400..2_592_000 => (Key::HomeEditedDay, age_secs / 86_400),
        2_592_000..31_536_000 => (Key::HomeEditedMonth, age_secs / 2_592_000),
        _ => (Key::HomeEditedYear, age_secs / 31_536_000),
    };
    t(key).replace("{n}", &n.to_string())
}

/// What the thumbnail thread found out about one file.
pub enum Card {
    /// The file is gone (or cannot be looked at): Home drops it.
    Missing,
    Ready {
        /// `(w, h, RGBA8)` of the file's `THUM` section; none in files saved before they had one.
        thumb: Option<(u16, u16, Vec<u8>)>,
        modified: Option<SystemTime>,
    },
}

/// Look at one file: its time and, from the manifest alone, its thumbnail.
pub fn read_card(path: &Path) -> Card {
    let Ok(meta) = std::fs::metadata(path) else { return Card::Missing };
    let thumb = arty_io::read_info(path).ok().and_then(|info| info.thumb);
    Card::Ready { thumb, modified: meta.modified().ok() }
}

/// Reads the cards of a list of files in order on its own thread, for one
/// visit to Home. Dropping it stops the thread at the next file. It runs at
/// normal priority: twelve manifests are a few milliseconds of work, and a
/// lowered one would wait out a busy machine while Home shows placeholders.
pub struct Loader {
    rx: Receiver<(PathBuf, Card)>,
}

impl Loader {
    /// `wake` runs after every card (a repaint request).
    pub fn spawn(paths: Vec<PathBuf>, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx): (Sender<(PathBuf, Card)>, _) = channel();
        let work = move || {
            for path in paths {
                let card = read_card(&path);
                if tx.send((path, card)).is_err() {
                    return;
                }
                wake();
            }
        };
        if let Err(e) = std::thread::Builder::new().name("arty-recent".into()).spawn(work) {
            log::warn!("recent files: {e}");
        }
        Self { rx }
    }

    pub fn try_recv(&self) -> Option<(PathBuf, Card)> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn touch_moves_to_front_and_caps() {
        let mut r = Recent::default();
        for i in 0..15 {
            r.touch(&p(&format!("C:\\art\\{i}.arty")));
        }
        assert_eq!(r.paths().len(), MAX_RECENT);
        assert_eq!(r.paths()[0], p("C:\\art\\14.arty"));
        assert_eq!(r.paths()[MAX_RECENT - 1], p("C:\\art\\3.arty"), "the oldest three fell off");
        r.touch(&p("C:\\art\\5.arty"));
        assert_eq!(r.paths()[0], p("C:\\art\\5.arty"));
        assert_eq!(r.paths().len(), MAX_RECENT, "moved, not added");
        assert_eq!(r.paths().iter().filter(|q| **q == p("C:\\art\\5.arty")).count(), 1);
    }

    #[test]
    fn windows_paths_ignore_case_and_slash_kind() {
        let mut r = Recent::default();
        r.touch(&p("C:\\Art\\Page.arty"));
        r.touch(&p("C:\\art\\other.arty"));
        r.touch(&p("c:/art/PAGE.arty"));
        if cfg!(windows) {
            assert_eq!(r.paths(), [p("c:/art/PAGE.arty"), p("C:\\art\\other.arty")], "the newest spelling is kept");
        } else {
            assert_eq!(r.paths().len(), 3);
        }
        r.remove(&p("C:\\ART\\OTHER.arty"));
        assert_eq!(r.paths().len(), if cfg!(windows) { 1 } else { 3 });
    }

    #[test]
    fn saved_form_round_trips_and_is_cleaned() {
        let mut r = Recent::default();
        r.touch(&p("D:\\a.arty"));
        r.touch(&p("D:\\b.arty"));
        assert_eq!(r.to_saved(), ["D:\\b.arty", "D:\\a.arty"]);
        assert_eq!(Recent::from_saved(&r.to_saved()), r);
        // A hand-edited or old list: duplicates and too many entries.
        let messy: Vec<String> = (0..30).map(|i| format!("D:\\{}.arty", i % 20)).collect();
        let r = Recent::from_saved(&messy);
        assert_eq!(r.paths().len(), MAX_RECENT);
        assert_eq!(r.paths()[0], p("D:\\0.arty"), "order kept");
        assert!(Recent::from_saved(&[]).paths().is_empty());
    }

    #[test]
    fn bench_list_parses() {
        assert_eq!(parse_paths("a.arty; b.arty;;|c.arty"), Some(vec![p("a.arty"), p("b.arty"), p("c.arty")]));
        assert_eq!(parse_paths(" ; "), None);
    }

    #[test]
    fn edited_text_in_both_languages() {
        use crate::text::{Lang, lang_for_test, set_current_lang};
        let _lang = lang_for_test(Lang::En);
        let cases: [(u64, &str, &str); 12] = [
            (0, "Edited just now", "เพิ่งแก้ไข"),
            (59, "Edited just now", "เพิ่งแก้ไข"),
            (60, "Edited 1 min ago", "แก้ไขเมื่อ 1 นาทีที่แล้ว"),
            (3599, "Edited 59 min ago", "แก้ไขเมื่อ 59 นาทีที่แล้ว"),
            (3600, "Edited 1 h ago", "แก้ไขเมื่อ 1 ชม.ที่แล้ว"),
            (2 * 3600 + 1800, "Edited 2 h ago", "แก้ไขเมื่อ 2 ชม.ที่แล้ว"),
            (86_400, "Edited 1 d ago", "แก้ไขเมื่อ 1 วันที่แล้ว"),
            (29 * 86_400, "Edited 29 d ago", "แก้ไขเมื่อ 29 วันที่แล้ว"),
            (30 * 86_400, "Edited 1 mo ago", "แก้ไขเมื่อ 1 เดือนที่แล้ว"),
            (364 * 86_400, "Edited 12 mo ago", "แก้ไขเมื่อ 12 เดือนที่แล้ว"),
            (365 * 86_400, "Edited 1 y ago", "แก้ไขเมื่อ 1 ปีที่แล้ว"),
            (u64::MAX, "Edited 584942417355 y ago", "แก้ไขเมื่อ 584942417355 ปีที่แล้ว"),
        ];
        for (secs, en, th) in cases {
            set_current_lang(Lang::En);
            assert_eq!(edited(secs), en, "{secs} s");
            set_current_lang(Lang::Th);
            assert_eq!(edited(secs), th, "{secs} s");
        }
    }

    /// A saved page gives its thumbnail and time from the manifest alone; a
    /// missing file and a file saved without a thumbnail are told apart.
    #[test]
    fn card_reads_the_thumbnail_of_a_saved_page() {
        use arty_core::{Document, TileCoord};
        use arty_io::{Progress, SaveExtras, SaveOptions, Session, SessionId};

        let dir = std::env::temp_dir().join(format!("arty-recent-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        let mut doc = Document::new(512, 1024, 72);
        let id = doc.active();
        let (grid, _) = doc.paint_target(id).unwrap();
        let mut tile = arty_core::tile::new_tile_box();
        for row in tile.iter_mut() {
            row.fill([arty_core::fix15::ONE_U16, 0, 0, arty_core::fix15::ONE_U16]);
        }
        grid.insert(TileCoord::new(0, 0), tile.into());

        let save = |name: &str, thumb: bool| {
            let path = dir.join(name);
            let ex = SaveExtras { thumb: thumb.then(|| arty_io::thumb::make(&doc, &pool)).flatten(), ..Default::default() };
            Session::new(SessionId([5; 16]), None)
                .save_main(&doc, &ex, &path, false, &SaveOptions::default(), &pool, &Progress::default())
                .unwrap();
            path
        };
        let with = save("with.arty", true);
        let Card::Ready { thumb: Some((w, h, px)), modified } = read_card(&with) else { panic!("a thumbnail") };
        assert_eq!((w, h, px.len()), (128, 256, 128 * 256 * 4), "a 512 x 1024 page fits 256 px");
        assert_eq!(&px[..4], [255, 0, 0, 255], "the red tile is at the top left");
        assert_eq!(&px[(255 * 128 + 127) * 4..][..4], [255, 255, 255, 255]);
        assert!(modified.is_some_and(|m| m.elapsed().unwrap_or_default() < Duration::from_secs(600)));

        assert!(matches!(read_card(&save("without.arty", false)), Card::Ready { thumb: None, .. }));
        assert!(matches!(read_card(&dir.join("gone.arty")), Card::Missing));
        std::fs::write(dir.join("junk.arty"), b"not an arty file").unwrap();
        assert!(matches!(read_card(&dir.join("junk.arty")), Card::Ready { thumb: None, .. }), "unreadable: a card without a picture");

        // The loader answers each path in order, then ends.
        let paths = vec![with.clone(), dir.join("gone.arty"), dir.join("without.arty")];
        let loader = Loader::spawn(paths.clone(), || {});
        let mut got = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while got.len() < paths.len() && std::time::Instant::now() < deadline {
            match loader.try_recv() {
                Some((path, card)) => got.push((path, matches!(card, Card::Missing))),
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        }
        assert_eq!(got, [(paths[0].clone(), false), (paths[1].clone(), true), (paths[2].clone(), false)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
