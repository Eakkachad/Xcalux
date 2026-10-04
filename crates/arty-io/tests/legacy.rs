//! R7: v1 files written the way `legacy/src/save.rs` writes them import
//! with the tree reversed and repaired, blend modes mapped, pixels intact
//! and duplicated layers sharing tiles; damaged tiles fail or are
//! salvaged; overwriting a v1 file keeps a backup.

mod common;

use std::sync::Arc;

use arty_core::{BlendMode, LayerId, PAPER_WHITE, TileCoord, TileRef};
use arty_io::{FileKind, IoError, LoadOptions, LoadWarning, Progress, SaveExtras, Session};
use common::v1::{LayerMetadata, SaveTask, TileSaveData, VectorControlPoint, VectorStroke, offsets, perform_save};
use common::*;

fn put(task: &mut SaveTask, layer_id: u32, tx: i32, ty: i32, pixels: &TileRef) {
    task.tiles.push(TileSaveData { layer_id, tx, ty, pixels: pixels.clone() });
}

fn strokes(n: usize) -> Vec<VectorStroke> {
    let p = VectorControlPoint { x: 1.5, y: 2.25, pressure: 0.5, tilt_x: 0.0, tilt_y: -0.125 };
    (0..n)
        .map(|i| VectorStroke { control_points: vec![p; 8], brush_preset_id: i as u64, color: [0.1, 0.2, 0.3], width: 4.0 })
        .collect()
}

/// Every feature of v1 the importer must handle, with the pixels each
/// imported layer should hold.
struct Sample {
    task: SaveTask,
    ink: TileRef,
    flat: TileRef,
    solid: TileRef,
    vector: TileRef,
}

fn sample() -> Sample {
    let mut rng = Rng(0x5EED_0001);
    let (ink, flat, vector) = (tile(&mut rng, 4), tile(&mut rng, 3), tile(&mut rng, 5));
    let solid = filled([ONE / 2, ONE / 3, ONE / 4, ONE]);
    let meta = |id, kind: &str, f: &dyn Fn(&mut LayerMetadata)| {
        let mut m = LayerMetadata::new(id, kind);
        f(&mut m);
        m
    };
    let layers_meta = vec![
        meta(10, "Raster", &|m| m.blend_mode = "Luminosity".into()),
        meta(4, "Folder", &|m| {
            m.blend_mode = "Shade".into();
            m.folder_child_ids = vec![5, 6, 11];
        }),
        meta(7, "Vector", &|m| m.vector_strokes = Some(strokes(3))),
        meta(8, "Vector", &|m| {
            m.visible = false;
            m.vector_strokes = Some(strokes(20_000));
        }),
        meta(5, "Raster", &|m| m.is_clipping = true),
        meta(2, "Raster", &|m| {
            m.opacity = 0.5;
            m.lock_alpha = true;
            m.blend_mode = "Multiply".into();
        }),
        meta(1, "Raster", &|m| m.blend_mode = "Mystery".into()),
        meta(6, "Raster", &|m| m.opacity = f32::NAN),
        meta(11, "Folder", &|m| m.folder_child_ids = vec![12]),
        meta(12, "Folder", &|m| m.folder_child_ids = vec![13]),
        meta(13, "Folder", &|m| m.folder_child_ids = vec![14]),
        meta(14, "Raster", &|_| {}),
        // Listed nowhere.
        meta(20, "Raster", &|m| m.name = "Orphan".into()),
    ];
    let mut task = SaveTask {
        canvas_width: 300,
        canvas_height: 200,
        // 5 is also a child of folder 4; 99 has no metadata.
        layer_order: vec![10, 4, 7, 8, 5, 2, 1, 99],
        layers_meta,
        tiles: Vec::new(),
    };
    // Layer 2 is a v1 duplicate of layer 1: equal bytes, separate tiles.
    for id in [1, 2] {
        let copy = |t: &TileRef| Arc::new(**t);
        put(&mut task, id, 0, 0, &copy(&flat));
        put(&mut task, id, 1, 0, &copy(&solid));
        put(&mut task, id, 2, 0, &copy(&solid));
    }
    put(&mut task, 10, 0, 0, &flat);
    put(&mut task, 10, 0, 0, &ink); // the later copy wins
    put(&mut task, 10, -3, 50, &ink); // past the page edge: kept
    put(&mut task, 7, 1, 1, &vector);
    put(&mut task, 99, 0, 0, &ink); // no such layer
    put(&mut task, 4, 0, 0, &ink); // a folder
    put(&mut task, 14, 0, 1, &solid);
    Sample { task, ink, flat, solid, vector }
}

fn import(bytes: &[u8], o: &LoadOptions) -> Result<arty_io::Loaded, IoError> {
    arty_io::load_from(bytes, None, o, &pool(), &Progress::default())
}

fn grid_tile(l: &arty_io::Loaded, id: u32, x: i32, y: i32) -> TileRef {
    l.doc.layer(LayerId(id)).unwrap().raster().unwrap().get_ref(TileCoord::new(x, y)).unwrap().clone()
}

#[test]
fn v1_files_import_with_repairs_and_warnings() {
    let s = sample();
    let bytes = perform_save(&s.task);
    let l = import(&bytes, &LoadOptions::default()).unwrap();
    let doc = &l.doc;

    // Top → bottom becomes bottom → top; the unlisted layer goes on top.
    let ids = |v: &[LayerId]| v.iter().map(|id| id.0).collect::<Vec<_>>();
    assert_eq!(ids(doc.root()), [1, 2, 8, 7, 4, 10, 20]);
    assert_eq!(ids(doc.layer(LayerId(4)).unwrap().children().unwrap()), [11, 6, 5]);
    for (folder, child) in [(11, 12), (12, 13), (13, 14)] {
        assert_eq!(ids(doc.layer(LayerId(folder)).unwrap().children().unwrap()), [child]);
    }
    assert_eq!((doc.width(), doc.height(), doc.dpi()), (300, 200, 350));
    assert_eq!(doc.paper(), Some(PAPER_WHITE));
    assert_eq!(doc.active(), LayerId(20), "topmost raster");
    assert_eq!(doc.next_layer_id(), 21);
    assert_eq!(l.info.kind, FileKind::LegacyV1);
    assert_eq!(l.read_only_reason, None);

    let props = |id| doc.layer(LayerId(id)).unwrap().props.clone();
    assert_eq!(props(10).blend, BlendMode::Add);
    assert_eq!(props(4).blend, BlendMode::LinearBurn, "folders keep their mapped mode");
    assert_eq!(props(1).blend, BlendMode::Normal);
    assert_eq!((props(2).blend, props(2).opacity, props(2).lock_alpha), (BlendMode::Multiply, 0.5, true));
    assert_eq!(props(6).opacity, 1.0, "NaN (saved as null) becomes opaque");
    assert!(props(5).clip && !props(8).visible);
    assert!(doc.layer(LayerId(7)).unwrap().raster().is_some(), "vector layers become raster layers");
    assert!(doc.layer(LayerId(8)).unwrap().raster().unwrap().is_empty());
    for id in [1, 2, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 20] {
        assert!(!props(id).locked, "layer {id}");
    }
    let expanded = |id| matches!(doc.layer(LayerId(id)).unwrap().content, arty_core::LayerContent::Folder { expanded: true, .. });
    assert!(expanded(4) && expanded(13));

    // Pixels, and sharing restored between v1's deep copies.
    assert!(*grid_tile(&l, 10, 0, 0) == *s.ink);
    assert!(*grid_tile(&l, 10, -3, 50) == *s.ink);
    assert!(*grid_tile(&l, 7, 1, 1) == *s.vector);
    assert!(*grid_tile(&l, 1, 0, 0) == *s.flat);
    assert!(*grid_tile(&l, 14, 0, 1) == *s.solid);
    assert!(Arc::ptr_eq(&grid_tile(&l, 1, 0, 0), &grid_tile(&l, 2, 0, 0)), "equal bytes share one Arc");
    let solids = [grid_tile(&l, 1, 1, 0), grid_tile(&l, 1, 2, 0), grid_tile(&l, 2, 1, 0), grid_tile(&l, 14, 0, 1)];
    assert!(solids.iter().all(|t| Arc::ptr_eq(t, &solids[0])), "one Arc per SOLID value");
    assert_eq!(doc.layer(LayerId(10)).unwrap().raster().unwrap().len(), 2);

    let expected = [
        LoadWarning::LegacyDuplicateRef { layer: 5 },
        LoadWarning::LegacyOrphan { layer: 20 },
        LoadWarning::LegacyDeepFolders { count: 1 },
        LoadWarning::LegacyDroppedTiles { count: 1, reason: "their layer is missing" },
        LoadWarning::LegacyDroppedTiles { count: 1, reason: "they belong to a folder" },
        LoadWarning::LegacyDroppedTiles { count: 1, reason: "stored twice; the last copy was kept" },
        LoadWarning::LegacyBlendMapped { layer: 10, from: "Luminosity".into() },
        LoadWarning::LegacyBlendMapped { layer: 4, from: "Shade".into() },
        LoadWarning::LegacyUnknownBlend { name: "Mystery".into() },
        LoadWarning::OpacityFixed { layer: 6 },
        LoadWarning::LegacyVectorRasterized { name: "Vector 7".into() },
        LoadWarning::LegacyVectorRasterized { name: "Vector 8".into() },
        LoadWarning::LegacyDefaultDpi,
    ];
    for w in &expected {
        assert!(l.warnings.contains(w), "{w:?} missing from {:?}", l.warnings);
    }
    assert_eq!(l.warnings.len(), expected.len(), "{:?}", l.warnings);

    let o = LoadOptions { legacy_dpi: 600, ..Default::default() };
    assert_eq!(import(&bytes, &o).unwrap().doc.dpi(), 600);
}

#[test]
fn writer_mirrors_perform_save() {
    let mut task = SaveTask { canvas_width: 64, canvas_height: 64, layer_order: vec![1], ..Default::default() };
    let mut m = LayerMetadata::new(1, "Raster");
    m.name = "a\"b".into();
    task.layers_meta.push(m);
    put(&mut task, 1, -1, 2, &filled([1, 2, 3, 4]));
    let f = perform_save(&task);
    let (json_off, dir_off) = offsets(&f);
    assert_eq!(&f[..8], b"ARTY\x01\0\0\0");
    assert!(json_off > 24, "tiles come first");
    assert_eq!(
        std::str::from_utf8(&f[json_off..dir_off]).unwrap(),
        r#"{"canvas_width":64,"canvas_height":64,"layer_order":[1],"layers":[{"id":1,"name":"a\"b","opacity":1.0,"visible":true,"lock_alpha":false,"is_clipping":false,"blend_mode":"Normal","kind":"Raster","folder_child_ids":[],"vector_strokes":null}]}"#
    );
    let dir = &f[dir_off..];
    assert_eq!(dir.len(), 24);
    assert_eq!(&dir[..12], [1u32.to_le_bytes(), (-1i32).to_le_bytes(), 2i32.to_le_bytes()].concat());
    assert_eq!(u64::from_le_bytes(dir[12..20].try_into().unwrap()), 24);
    assert_eq!(u32::from_le_bytes(dir[20..24].try_into().unwrap()) as usize, json_off - 24);
    let l = import(&f, &LoadOptions::default()).unwrap();
    assert!(*grid_tile(&l, 1, -1, 2) == *filled([1, 2, 3, 4]));
}

#[test]
fn damaged_v1_tiles_fail_unless_salvaged() {
    let s = sample();
    let good = perform_save(&s.task);
    let (json_off, dir_off) = offsets(&good);
    let salvage = LoadOptions { salvage: true, ..Default::default() };

    // A broken deflate stream (first tile, layer 1 at (0, 0)).
    let mut bad = good.clone();
    bad[24..40].fill(0xFF);
    assert!(matches!(import(&bad, &LoadOptions::default()), Err(IoError::Corrupt { what: "v1 tile data", .. })));
    let l = import(&bad, &salvage).unwrap();
    assert!(l.warnings.contains(&LoadWarning::DamagedTiles { count: 1 }));
    assert!(l.read_only_reason.is_some());
    assert!(l.doc.layer(LayerId(1)).unwrap().raster().unwrap().get(TileCoord::new(0, 0)).is_none());
    assert!(*grid_tile(&l, 2, 0, 0) == *s.flat, "other tiles still load");

    // A compressed size over 1 MiB, and one running into the JSON.
    for csize in [(1u32 << 20) + 1, json_off as u32] {
        let mut bad = good.clone();
        bad[dir_off + 20..dir_off + 24].copy_from_slice(&csize.to_le_bytes());
        assert!(matches!(import(&bad, &LoadOptions::default()), Err(IoError::Corrupt { what: "v1 tile range", .. })));
        assert!(import(&bad, &salvage).unwrap().warnings.contains(&LoadWarning::DamagedTiles { count: 1 }));
    }

    // Bad header offsets and metadata are refused outright.
    let mut bad = good.clone();
    bad[8..16].copy_from_slice(&(good.len() as u64 + 1).to_le_bytes());
    assert!(matches!(import(&bad, &salvage), Err(IoError::Corrupt { what: "v1 header offsets", .. })));
    let mut bad = good.clone();
    bad[json_off] = b'[';
    assert!(matches!(import(&bad, &salvage), Err(IoError::Corrupt { what: "v1 metadata", .. })));
    // A partial directory entry at the end is only a warning.
    let mut tail = good.clone();
    tail.extend_from_slice(&[0; 7]);
    let l = import(&tail, &LoadOptions::default()).unwrap();
    assert!(l.warnings.contains(&LoadWarning::LegacyDroppedTiles { count: 1, reason: "incomplete directory entry" }));
    // Budget: the tiles are counted before any is decoded.
    let small = LoadOptions {
        limits: arty_io::LoadLimits { max_decoded_bytes: 4 * 32768, ..Default::default() },
        ..Default::default()
    };
    assert!(matches!(import(&good, &small), Err(IoError::LimitExceeded { what: "decoded pixels", .. })));
}

#[test]
fn a_v1_file_without_rasters_gets_one() {
    let task = SaveTask {
        canvas_width: 10,
        canvas_height: 10,
        layer_order: vec![3],
        layers_meta: vec![LayerMetadata::new(3, "Folder")],
        tiles: Vec::new(),
    };
    let l = import(&perform_save(&task), &LoadOptions::default()).unwrap();
    assert!(l.warnings.contains(&LoadWarning::AddedMissingRaster));
    assert_eq!(l.doc.root(), [LayerId(3), LayerId(4)]);
    assert_eq!(l.doc.active(), LayerId(4));
    let empty = SaveTask { canvas_width: 0, canvas_height: 10, ..task };
    assert!(matches!(import(&perform_save(&empty), &LoadOptions::default()), Err(IoError::Corrupt { what: "page size", .. })));
}

#[test]
fn saving_over_a_v1_file_keeps_a_backup() {
    let pool = pool();
    let dir = temp_dir("legacy-backup");
    let path = dir.join("page.arty");
    let v1 = perform_save(&sample().task);
    std::fs::write(&path, &v1).unwrap();

    let info = arty_io::read_info(&path).unwrap();
    assert_eq!((info.kind, info.width, info.height, info.layer_count), (FileKind::LegacyV1, 300, 200, 13));
    let mut loaded = arty_io::load(&path, &LoadOptions::default(), &pool, &Progress::default()).unwrap();
    let mut session = Session::new(session(), None);
    session.adopt(&mut loaded, None);
    let ex = SaveExtras::default();
    let stats = session.save_main(&loaded.doc, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!(stats.classified, 0, "the import already classified every tile");
    assert_eq!(std::fs::read(dir.join("page.v1-backup.arty")).unwrap(), v1);
    let back = arty_io::load(&path, &LoadOptions::default(), &pool, &Progress::default()).unwrap();
    assert_eq!(back.info.kind, FileKind::V2 { minor: 0 });
    assert_same_doc(&loaded.doc, &back.doc);

    // A second v1 file at the same path does not overwrite the first backup.
    std::fs::write(&path, &v1).unwrap();
    let r = session.save_main(&loaded.doc, &ex, &path, false, &opts(), &pool, &Progress::default());
    assert!(matches!(r, Err(IoError::ExternallyModified)));
    session.save_main(&loaded.doc, &ex, &path, true, &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!(std::fs::read(dir.join("page.v1-backup-1.arty")).unwrap(), v1);
    std::fs::remove_dir_all(&dir).unwrap();
}
