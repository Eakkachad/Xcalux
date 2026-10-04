//! Print the structure of an `.arty` file: header, records, commits, the
//! newest manifest's sections and layers, and per-layer tile table stats.
//!
//! cargo run -p arty-io --example arty_dump -- FILE [--records]
//!
//! `--records` lists every record (otherwise only totals per kind). Reads
//! headers and tables only, never pixels, and stops at the first damaged
//! record header. v1 files show their header, JSON and directory sizes.

use std::collections::BTreeMap;
use std::fs::File;

use arty_io::ReadAt;
use arty_io::format::{
    COMMIT_RECORD_LEN, Commit, HEADER_LEN, Header, RECORD_HEADER_LEN, RecordHeader, RecordKind, TileCodec, unpack_solid,
};
use arty_io::limits::MAX_LAYER_COUNT;
use arty_io::{FileKind, manifest, names, sniff, table};

fn read(f: &File, at: u64, n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    f.read_exact_at(&mut b, at).unwrap_or_else(|e| panic!("reading {n} bytes at {at}: {e}"));
    b
}

fn kind_name(k: u8) -> &'static str {
    match RecordKind::from_u8(k) {
        Some(RecordKind::Segment) => "Segment",
        Some(RecordKind::Manifest) => "Manifest",
        Some(RecordKind::Commit) => "Commit",
        Some(RecordKind::TileTable) => "TileTable",
        None => "unknown",
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: arty_dump FILE [--records]");
    let list_records = args.any(|a| a == "--records");
    let f = File::open(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let len = f.len().unwrap();
    let head = read(&f, 0, len.min(HEADER_LEN as u64) as usize);
    println!("{path}: {len} bytes");
    match sniff(&head) {
        FileKind::V2 { .. } => dump_v2(&f, len, &head, list_records),
        FileKind::LegacyV1 => dump_v1(&f, len),
        other => println!("not a readable .arty file: {other:?}"),
    }
}

fn dump_v1(f: &File, len: u64) {
    let h = read(f, 0, 24);
    let at = |i: usize| u64::from_le_bytes(h[i..i + 8].try_into().unwrap());
    let (json_off, dir_off) = (at(8), at(16));
    println!("v1 file: tiles [24, {json_off}), JSON [{json_off}, {dir_off}), directory [{dir_off}, {len})");
    let entries = len.saturating_sub(dir_off) / 24;
    println!("  {} JSON bytes, {entries} directory entries", dir_off.saturating_sub(json_off));
    #[cfg(feature = "legacy")]
    if json_off <= dir_off && dir_off <= len && dir_off - json_off <= 64 << 20 {
        let json = read(f, json_off, (dir_off - json_off) as usize);
        match serde_json::from_slice::<serde_json::Value>(&json) {
            Ok(v) => {
                println!("  canvas {} x {}", v["canvas_width"], v["canvas_height"]);
                println!("  layer_order (top first) {}", v["layer_order"]);
                for l in v["layers"].as_array().into_iter().flatten() {
                    println!(
                        "  layer {:>4} {:<8} blend {:<10} opacity {} children {} {}",
                        l["id"],
                        l["kind"].as_str().unwrap_or("?"),
                        l["blend_mode"].as_str().unwrap_or("?"),
                        l["opacity"],
                        l["folder_child_ids"],
                        l["name"]
                    );
                }
            }
            Err(e) => println!("  JSON does not parse: {e}"),
        }
    }
    let mut per_layer: BTreeMap<u32, (u64, u64)> = BTreeMap::new();
    let dir = read(f, dir_off.min(len), (entries * 24) as usize);
    for e in dir.chunks_exact(24) {
        let layer = u32::from_le_bytes(e[..4].try_into().unwrap());
        let csize = u32::from_le_bytes(e[20..24].try_into().unwrap());
        let s = per_layer.entry(layer).or_default();
        s.0 += 1;
        s.1 += u64::from(csize);
    }
    for (layer, (n, bytes)) in per_layer {
        println!("  tiles of layer {layer:>4}: {n:>7}, {bytes:>11} deflate bytes");
    }
}

fn dump_v2(f: &File, len: u64, head: &[u8], list_records: bool) {
    let h = match Header::decode(head) {
        Ok(h) => h,
        Err(e) => return println!("bad header: {e}"),
    };
    let uuid: String = h.file_uuid.iter().map(|b| format!("{b:02x}")).collect();
    println!(
        "v2.{} header: required 0x{:x}, optional 0x{:x}{}, creator {}.{}.{}, uuid {uuid}",
        h.minor,
        h.required_flags,
        h.optional_flags,
        if h.optional_flags & 1 != 0 { " (recovery file)" } else { "" },
        h.creator >> 16,
        (h.creator >> 8) & 0xFF,
        h.creator & 0xFF
    );

    // Records, forward.
    let mut totals: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    let mut commits = Vec::new();
    let mut pos = HEADER_LEN as u64;
    while pos < len {
        let Some(rh) = (pos + RECORD_HEADER_LEN as u64 <= len).then(|| read(f, pos, RECORD_HEADER_LEN)) else {
            println!("truncated record header at {pos}");
            break;
        };
        let r = match RecordHeader::decode(&rh, pos) {
            Ok(r) => r,
            Err(e) => {
                println!("stopped at {pos}: {e}");
                break;
            }
        };
        let end = match r.end(pos) {
            Ok(end) if end <= len => end,
            _ => {
                println!("record at {pos} runs past the end ({} payload bytes)", r.payload_len);
                break;
            }
        };
        let name = kind_name(r.kind);
        if list_records {
            println!("  {pos:>12} {name:<9} {:>10} bytes", r.payload_len);
        }
        let t = totals.entry(name).or_default();
        t.0 += 1;
        t.1 += end - pos;
        if r.kind == RecordKind::Commit as u8 && end - pos == COMMIT_RECORD_LEN as u64 {
            match Commit::decode_record(&read(f, pos, COMMIT_RECORD_LEN), pos) {
                Ok(c) => commits.push((pos, c)),
                Err(e) => println!("  bad commit at {pos}: {e}"),
            }
        }
        pos = end;
    }
    for (name, (n, bytes)) in &totals {
        println!("{name:<9} {n:>7} records {bytes:>13} bytes ({:.1}%)", *bytes as f64 * 100.0 / len as f64);
    }
    for (at, c) in &commits {
        println!(
            "commit #{} at {at}: manifest {}, previous {}, {} ms",
            c.commit_seq, c.manifest_offset, c.prev_commit_offset, c.unix_ms
        );
    }
    let Some(&(_, c)) = commits.last() else { return println!("no commit found") };

    // The newest commit's manifest.
    let mh = RecordHeader::decode(&read(f, c.manifest_offset, RECORD_HEADER_LEN), c.manifest_offset).unwrap();
    let payload = read(f, c.manifest_offset + RECORD_HEADER_LEN as u64, mh.payload_len as usize);
    let raw = match manifest::decode_payload(&payload, c.manifest_offset) {
        Ok(raw) => raw,
        Err(e) => return println!("manifest: {e}"),
    };
    println!("manifest: {} stored bytes, {} decoded", payload.len(), raw.len());
    let mut sec = &raw[..];
    while sec.len() >= 12 {
        let flags = u32::from_le_bytes(sec[4..8].try_into().unwrap());
        let n = u32::from_le_bytes(sec[8..12].try_into().unwrap()) as usize;
        println!("  section {:?} flags 0x{flags:x} {n} bytes", String::from_utf8_lossy(&sec[..4]));
        sec = &sec[(12 + n).min(sec.len())..];
    }
    let m = match manifest::parse(&raw, c.manifest_offset, MAX_LAYER_COUNT) {
        Ok(m) => m,
        Err(e) => return println!("manifest: {e}"),
    };
    let d = &m.doc;
    println!(
        "DOC {} x {} at {} dpi, paper {:?}, active {}, next id {}, {} layers",
        d.width, d.height, d.dpi, d.paper, d.active, d.next_id, d.layer_count
    );
    for (k, v) in &m.meta {
        println!("META {k} = {v}");
    }
    for w in &m.warnings {
        println!("warning: {w}");
    }

    // Layers and their tables.
    for l in &m.layers {
        let blend = names::blend_from_id(l.blend).map_or_else(|| format!("#{}", l.blend), |b| format!("{b:?}"));
        print!(
            "layer {:>5} parent {:>5} kind {} flags 0x{:02x} {:<11} opacity {:<6} {:?}",
            l.id,
            l.parent_id,
            l.kind,
            l.flags,
            blend,
            f32::from_bits(l.opacity_bits),
            String::from_utf8_lossy(l.name)
        );
        if l.tile_count == 0 {
            println!();
            continue;
        }
        let th = RecordHeader::decode(&read(f, l.table_offset, RECORD_HEADER_LEN), l.table_offset).unwrap();
        let p = read(f, l.table_offset + RECORD_HEADER_LEN as u64, th.payload_len as usize);
        let entries = match table::parse(&p, l.table_offset, l.id, l.tile_count) {
            Ok(e) => e,
            Err(e) => {
                println!("\n  table: {e}");
                continue;
            }
        };
        let mut codecs: BTreeMap<&str, (u32, u64)> = BTreeMap::new();
        let mut solids = std::collections::BTreeSet::new();
        for e in &entries {
            let name = match e.codec {
                TileCodec::Raw => "raw",
                TileCodec::Lz4Shuf => "shuf",
                TileCodec::Lz4Vdelta => "vdelta",
                TileCodec::Solid => {
                    solids.insert(unpack_solid(e.offset));
                    "solid"
                }
            };
            let s = codecs.entry(name).or_default();
            s.0 += 1;
            s.1 += u64::from(e.stored_len);
        }
        let stats: Vec<String> = codecs.iter().map(|(k, (n, b))| format!("{k} {n} ({b} B)")).collect();
        println!(
            "\n  table at {}: {} tiles, {} table bytes; {}; {} distinct solid values",
            l.table_offset,
            entries.len(),
            p.len(),
            stats.join(", "),
            solids.len()
        );
    }
}
