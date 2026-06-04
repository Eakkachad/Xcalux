use crate::canvas::{BlendMode, Layer, LayerType, Tile};
use ahash::AHashMap;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use std::io::{Read, Seek, Write};
use std::path::Path;

/// Convert internal fix15 (0..=32768) to 8-bit (0..=255)
#[inline]
pub fn fix15_to_u8(v: u16) -> u8 {
    ((v as u32 * 255 + 16384) >> 15) as u8
}

/// Convert 8-bit (0..=255) to internal fix15 (0..=32768)
#[inline]
pub fn u8_to_fix15(v: u8) -> u16 {
    ((v as u32 * 32768 + 127) / 255) as u16
}

/// Map internal BlendMode to SVG composite/blend name
pub fn blend_mode_to_svg(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Normal => "svg:src-over",
        BlendMode::Multiply => "svg:multiply",
        BlendMode::Screen => "svg:screen",
        BlendMode::Overlay => "svg:overlay",
        BlendMode::Luminosity | BlendMode::Shade => "svg:src-over",
    }
}

/// Parse an SVG blend mode name back to internal BlendMode
pub fn svg_to_blend_mode(s: &str) -> BlendMode {
    match s {
        "svg:multiply" => BlendMode::Multiply,
        "svg:screen" => BlendMode::Screen,
        "svg:overlay" => BlendMode::Overlay,
        _ => BlendMode::Normal,
    }
}

/// Flatten a layer's sparse tiles into a bounded RGBA buffer
pub fn flatten_layer_rgba(layer: &Layer, canvas_w: u32, canvas_h: u32) -> Vec<u8> {
    let mut img: Vec<u8> = vec![0u8; (canvas_w * canvas_h * 4) as usize];
    for (&(tx, ty), tile) in &layer.tiles {
        for ly in 0usize..64 {
            for lx in 0usize..64 {
                let wx = (tx * 64 + lx as i32) as u32;
                let wy = (ty * 64 + ly as i32) as u32;
                if wx >= canvas_w || wy >= canvas_h {
                    continue;
                }
                let idx = ((wy * canvas_w + wx) * 4) as usize;
                let p = tile.pixels[ly][lx];
                img[idx] = fix15_to_u8(p[0]);
                img[idx + 1] = fix15_to_u8(p[1]);
                img[idx + 2] = fix15_to_u8(p[2]);
                img[idx + 3] = fix15_to_u8(p[3]);
            }
        }
    }
    img
}

/// Encode an RGBA buffer to PNG bytes
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Decode a PNG byte buffer into an RGBA buffer
pub fn decode_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let bytes = &buf[..info.buffer_size()];
    let mut rgba = Vec::with_capacity((info.width * info.height * 4) as usize);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(bytes),
        png::ColorType::Rgb => {
            for chunk in bytes.chunks(3) {
                rgba.push(chunk[0]);
                rgba.push(chunk[1]);
                rgba.push(chunk[2]);
                rgba.push(255);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for chunk in bytes.chunks(2) {
                rgba.push(chunk[0]);
                rgba.push(chunk[0]);
                rgba.push(chunk[0]);
                rgba.push(chunk[1]);
            }
        }
        png::ColorType::Grayscale => {
            for &v in bytes {
                rgba.push(v);
                rgba.push(v);
                rgba.push(v);
                rgba.push(255);
            }
        }
        _ => return Err(format!("Unsupported PNG color type: {:?}", info.color_type)),
    }
    Ok((info.width, info.height, rgba))
}

/// Build an OpenRaster stack.xml from the layer order
pub fn build_stack_xml(layers: &AHashMap<u32, Layer>, layer_order: &[u32], width: u32, height: u32) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(&format!("<image width=\"{}\" height=\"{}\" xres=\"72\" yres=\"72\" version=\"0.0.5\">\n", width, height));
    xml.push_str("  <stack>\n");
    for (i, &layer_id) in layer_order.iter().enumerate() {
        let layer = match layers.get(&layer_id) {
            Some(l) => l,
            None => continue,
        };
        let visibility = if layer.visible { "visible" } else { "hidden" };
        let blend = blend_mode_to_svg(layer.blend_mode);
        let escaped_name = layer.name.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        xml.push_str(&format!(
            "    <layer name=\"{}\" opacity=\"{:.4}\" visibility=\"{}\" blendmode=\"{}\" src=\"data/layer{}.png\"/>\n",
            escaped_name, layer.opacity, visibility, blend, i
        ));
    }
    xml.push_str("  </stack>\n");
    xml.push_str("</image>\n");
    xml
}

/// Parse OpenRaster stack.xml and extract canvas dimensions and layer entries
#[derive(Debug, Clone)]
pub struct OraLayerEntry {
    pub name: String,
    pub opacity: f32,
    pub visible: bool,
    pub blend_mode: BlendMode,
    pub src: String,
}

#[derive(Debug, Clone)]
pub struct OraStack {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<OraLayerEntry>,
}

/// Simple XML scanner - extracts attribute values from <layer> elements
pub fn parse_stack_xml(xml: &str) -> Result<OraStack, String> {
    let width_start = xml.find("width=\"").ok_or("Missing width attribute")?;
    let width_str = &xml[width_start + 7..];
    let width_end = width_str.find('"').ok_or("Unterminated width")?;
    let width: u32 = width_str[..width_end].parse().map_err(|e| format!("Bad width: {}", e))?;

    let height_start = xml.find("height=\"").ok_or("Missing height attribute")?;
    let height_str = &xml[height_start + 8..];
    let height_end = height_str.find('"').ok_or("Unterminated height")?;
    let height: u32 = height_str[..height_end].parse().map_err(|e| format!("Bad height: {}", e))?;

    let mut layers = Vec::new();
    let mut search_from = 0;
    while let Some(layer_start) = xml[search_from..].find("<layer ") {
        let abs_start = search_from + layer_start;
        let layer_end = xml[abs_start..].find("/>").ok_or("Unterminated layer tag")?;
        let abs_end = abs_start + layer_end;
        let layer_xml = &xml[abs_start..abs_end];

        let name = extract_attr(layer_xml, "name").unwrap_or_else(|| "Layer".to_string());
        let opacity: f32 = extract_attr(layer_xml, "opacity")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1.0);
        let visibility = extract_attr(layer_xml, "visibility").unwrap_or_else(|| "visible".to_string());
        let blend_mode_str = extract_attr(layer_xml, "blendmode").unwrap_or_else(|| "svg:src-over".to_string());
        let src = extract_attr(layer_xml, "src").unwrap_or_default();

        layers.push(OraLayerEntry {
            name,
            opacity: opacity.clamp(0.0, 1.0),
            visible: visibility == "visible",
            blend_mode: svg_to_blend_mode(&blend_mode_str),
            src,
        });

        search_from = abs_end + 2;
    }

    Ok(OraStack { width, height, layers })
}

fn extract_attr(xml: &str, attr: &str) -> Option<String> {
    let pat = format!("{}=\"", attr);
    let start = xml.find(&pat)? + pat.len();
    let rest = &xml[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Convert an RGBA buffer back into a sparse TileMap (only allocates non-empty tiles)
pub fn rgba_to_tiles(rgba: &[u8], width: u32, height: u32) -> AHashMap<(i32, i32), Tile> {
    let mut tiles: AHashMap<(i32, i32), Tile> = AHashMap::default();

    let ty_max = height.div_ceil(64);
    let tx_max = width.div_ceil(64);
    for ty in 0..ty_max as i32 {
        for tx in 0..tx_max as i32 {
            let mut has_content = false;
            let mut tile = Tile::new();
            for ly in 0usize..64 {
                for lx in 0usize..64 {
                    let wx = tx * 64 + lx as i32;
                    let wy = ty * 64 + ly as i32;
                    if wx < 0 || wy < 0 || wx >= width as i32 || wy >= height as i32 {
                        continue;
                    }
                    let idx = ((wy as u32 * width + wx as u32) * 4) as usize;
                    if idx + 3 >= rgba.len() {
                        continue;
                    }
                    let r = rgba[idx];
                    let g = rgba[idx + 1];
                    let b = rgba[idx + 2];
                    let a = rgba[idx + 3];
                    if a > 0 {
                        has_content = true;
                    }
                    tile.pixels[ly][lx] = [u8_to_fix15(r), u8_to_fix15(g), u8_to_fix15(b), u8_to_fix15(a)];
                }
            }
            if has_content {
                tiles.insert((tx, ty), tile);
            }
        }
    }
    tiles
}

/// Build a flattened composite RGBA buffer (used for mergedimage and thumbnail)
pub fn flatten_composite(layers: &AHashMap<u32, Layer>, layer_order: &[u32], canvas_w: u32, canvas_h: u32) -> Vec<u8> {
    let mut img: Vec<u8> = vec![0u8; (canvas_w * canvas_h * 4) as usize];

    for &layer_id in layer_order.iter().rev() {
        let layer = match layers.get(&layer_id) {
            Some(l) => l,
            None => continue,
        };
        if !layer.visible || layer.opacity <= 0.0 {
            continue;
        }
        let layer_opacity = layer.opacity;

        for (&(tx, ty), tile) in &layer.tiles {
            for ly in 0usize..64 {
                for lx in 0usize..64 {
                    let wx = (tx * 64 + lx as i32) as u32;
                    let wy = (ty * 64 + ly as i32) as u32;
                    if wx >= canvas_w || wy >= canvas_h {
                        continue;
                    }
                    let idx = ((wy * canvas_w + wx) * 4) as usize;
                    let p = tile.pixels[ly][lx];

                    let src_r = fix15_to_u8(p[0]);
                    let src_g = fix15_to_u8(p[1]);
                    let src_b = fix15_to_u8(p[2]);
                    let src_a = (fix15_to_u8(p[3]) as f32 * layer_opacity / 255.0).clamp(0.0, 1.0);

                    let dst_a = img[idx + 3] as f32 / 255.0;
                    let out_a = src_a + dst_a * (1.0 - src_a);
                    if out_a > 0.0 {
                        img[idx] = ((src_r as f32 * src_a + img[idx] as f32 * dst_a * (1.0 - src_a)) / out_a) as u8;
                        img[idx + 1] = ((src_g as f32 * src_a + img[idx + 1] as f32 * dst_a * (1.0 - src_a)) / out_a) as u8;
                        img[idx + 2] = ((src_b as f32 * src_a + img[idx + 2] as f32 * dst_a * (1.0 - src_a)) / out_a) as u8;
                        img[idx + 3] = (out_a * 255.0) as u8;
                    }
                }
            }
        }
    }

    img
}

/// Resize an RGBA buffer using nearest-neighbor (used for thumbnail)
pub fn resize_rgba_nn(src: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u8> {
    let mut dst = vec![0u8; (dst_w * dst_h * 4) as usize];
    for y in 0..dst_h {
        for x in 0..dst_w {
            let sx = (x * src_w / dst_w) as usize;
            let sy = (y * src_h / dst_h) as usize;
            let src_idx = (sy * src_w as usize + sx) * 4;
            let dst_idx = (y as usize * dst_w as usize + x as usize) * 4;
            if src_idx + 3 < src.len() && dst_idx + 3 < dst.len() {
                dst[dst_idx..dst_idx + 4].copy_from_slice(&src[src_idx..src_idx + 4]);
            }
        }
    }
    dst
}

// ============================================================================
// Minimal ZIP writer and reader (subset of PKZIP spec, sufficient for ORA)
// ============================================================================

const ZIP_LOCAL_HEADER: u32 = 0x04034b50;
const ZIP_CENTRAL_HEADER: u32 = 0x02014b50;
const ZIP_END_OF_CENTRAL: u32 = 0x06054b50;

#[derive(Debug, Clone)]
struct ZipEntryMeta {
    name: String,
    crc32: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    local_header_offset: u32,
    method: u16, // 0 = stored, 8 = deflated
}

struct ZipEntryWriter {
    name: String,
    crc: crc32fast::Hasher,
    uncompressed_size: u32,
    data_start_offset: u32,
    method: u16,
    compressor: Option<DeflateEncoder<Vec<u8>>>,
    buffered_data: Vec<u8>,
}

impl ZipEntryWriter {
    fn new_stored(name: String, offset: u32) -> Self {
        Self {
            name,
            crc: crc32fast::Hasher::new(),
            uncompressed_size: 0,
            data_start_offset: offset,
            method: 0,
            compressor: None,
            buffered_data: Vec::new(),
        }
    }

    fn new_deflated(name: String, offset: u32) -> Self {
        Self {
            name,
            crc: crc32fast::Hasher::new(),
            uncompressed_size: 0,
            data_start_offset: offset,
            method: 8,
            compressor: Some(DeflateEncoder::new(Vec::new(), Compression::default())),
            buffered_data: Vec::new(),
        }
    }

    fn write(&mut self, data: &[u8]) -> Result<(), String> {
        self.crc.update(data);
        self.uncompressed_size = self.uncompressed_size.wrapping_add(data.len() as u32);
        if let Some(enc) = self.compressor.as_mut() {
            enc.write_all(data).map_err(|e| e.to_string())?;
        } else {
            self.buffered_data.extend_from_slice(data);
        }
        Ok(())
    }

    fn finalize(mut self) -> Result<(Vec<u8>, ZipEntryMeta), String> {
        let compressed_data = if let Some(enc) = self.compressor.take() {
            enc.finish().map_err(|e| e.to_string())?
        } else {
            self.buffered_data
        };
        let meta = ZipEntryMeta {
            name: self.name,
            crc32: self.crc.finalize(),
            compressed_size: compressed_data.len() as u32,
            uncompressed_size: self.uncompressed_size,
            local_header_offset: self.data_start_offset,
            method: self.method,
        };
        Ok((compressed_data, meta))
    }
}

fn write_local_header(w: &mut Vec<u8>, name: &str, meta: &ZipEntryMeta) {
    let name_bytes = name.as_bytes();
    w.extend_from_slice(&ZIP_LOCAL_HEADER.to_le_bytes());
    w.extend_from_slice(&20u16.to_le_bytes()); // version needed
    w.extend_from_slice(&0u16.to_le_bytes()); // flags
    w.extend_from_slice(&meta.method.to_le_bytes());
    w.extend_from_slice(&0u16.to_le_bytes()); // mod time
    w.extend_from_slice(&0u16.to_le_bytes()); // mod date
    w.extend_from_slice(&meta.crc32.to_le_bytes());
    w.extend_from_slice(&meta.compressed_size.to_le_bytes());
    w.extend_from_slice(&meta.uncompressed_size.to_le_bytes());
    w.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    w.extend_from_slice(&0u16.to_le_bytes()); // extra field length
    w.extend_from_slice(name_bytes);
}

fn write_central_header(w: &mut Vec<u8>, name: &str, meta: &ZipEntryMeta) {
    let name_bytes = name.as_bytes();
    w.extend_from_slice(&ZIP_CENTRAL_HEADER.to_le_bytes());
    w.extend_from_slice(&20u16.to_le_bytes()); // version made by
    w.extend_from_slice(&20u16.to_le_bytes()); // version needed
    w.extend_from_slice(&0u16.to_le_bytes()); // flags
    w.extend_from_slice(&meta.method.to_le_bytes());
    w.extend_from_slice(&0u16.to_le_bytes()); // mod time
    w.extend_from_slice(&0u16.to_le_bytes()); // mod date
    w.extend_from_slice(&meta.crc32.to_le_bytes());
    w.extend_from_slice(&meta.compressed_size.to_le_bytes());
    w.extend_from_slice(&meta.uncompressed_size.to_le_bytes());
    w.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    w.extend_from_slice(&0u16.to_le_bytes()); // extra field length
    w.extend_from_slice(&0u16.to_le_bytes()); // comment length
    w.extend_from_slice(&0u16.to_le_bytes()); // disk number
    w.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
    w.extend_from_slice(&0u32.to_le_bytes()); // external attrs
    w.extend_from_slice(&meta.local_header_offset.to_le_bytes());
    w.extend_from_slice(name_bytes);
}

fn write_end_of_central(w: &mut Vec<u8>, entry_count: u16, central_dir_size: u32, central_dir_offset: u32) {
    w.extend_from_slice(&ZIP_END_OF_CENTRAL.to_le_bytes());
    w.extend_from_slice(&0u16.to_le_bytes()); // disk number
    w.extend_from_slice(&0u16.to_le_bytes()); // start disk
    w.extend_from_slice(&entry_count.to_le_bytes());
    w.extend_from_slice(&entry_count.to_le_bytes());
    w.extend_from_slice(&central_dir_size.to_le_bytes());
    w.extend_from_slice(&central_dir_offset.to_le_bytes());
    w.extend_from_slice(&0u16.to_le_bytes()); // comment length
}

/// Read u16 LE from a byte slice at a given offset
fn read_u16_le(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

/// Read u32 LE from a byte slice at a given offset
fn read_u32_le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[derive(Debug, Clone)]
struct ZipCentralEntry {
    name: String,
    #[allow(dead_code)]
    crc32: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    local_header_offset: u32,
    method: u16,
}

/// Find the End of Central Directory record by scanning from the end of the file
fn find_eocd(buf: &[u8]) -> Option<usize> {
    // EOCD minimum is 22 bytes; max comment is 65535, so scan at most 65557 bytes from end
    let start = buf.len().saturating_sub(22 + 65535);
    (start..=buf.len().saturating_sub(22)).rev().find(|&i| read_u32_le(buf, i) == ZIP_END_OF_CENTRAL)
}

/// Parse central directory entries from the ZIP archive
fn read_central_directory(buf: &[u8]) -> Result<Vec<ZipCentralEntry>, String> {
    let eocd_off = find_eocd(buf).ok_or("ZIP: EOCD not found")?;
    let entry_count = read_u16_le(buf, eocd_off + 10);
    let central_size = read_u32_le(buf, eocd_off + 12);
    let central_off = read_u32_le(buf, eocd_off + 16) as usize;

    let mut entries = Vec::with_capacity(entry_count as usize);
    let mut pos = central_off;
    let end = central_off + central_size as usize;
    while pos < end {
        if read_u32_le(buf, pos) != ZIP_CENTRAL_HEADER {
            return Err("ZIP: bad central header signature".to_string());
        }
        let method = read_u16_le(buf, pos + 10);
        let crc32 = read_u32_le(buf, pos + 16);
        let compressed_size = read_u32_le(buf, pos + 20);
        let uncompressed_size = read_u32_le(buf, pos + 24);
        let name_len = read_u16_le(buf, pos + 28) as usize;
        let extra_len = read_u16_le(buf, pos + 30) as usize;
        let comment_len = read_u16_le(buf, pos + 32) as usize;
        let local_off = read_u32_le(buf, pos + 42);
        let name_bytes = &buf[pos + 46..pos + 46 + name_len];
        let name = std::str::from_utf8(name_bytes)
            .map_err(|e| format!("ZIP: bad entry name: {}", e))?
            .to_string();
        entries.push(ZipCentralEntry {
            name,
            crc32,
            compressed_size,
            uncompressed_size,
            local_header_offset: local_off,
            method,
        });
        pos += 46 + name_len + extra_len + comment_len;
    }
    Ok(entries)
}

/// Read the data for a single ZIP entry, decompressing if necessary
fn read_entry_data(buf: &[u8], entry: &ZipCentralEntry) -> Result<Vec<u8>, String> {
    let pos = entry.local_header_offset as usize;
    if read_u32_le(buf, pos) != ZIP_LOCAL_HEADER {
        return Err(format!("ZIP: bad local header for {}", entry.name));
    }
    let name_len = read_u16_le(buf, pos + 26) as usize;
    let extra_len = read_u16_le(buf, pos + 28) as usize;
    let data_start = pos + 30 + name_len + extra_len;
    let data_end = data_start + entry.compressed_size as usize;
    if data_end > buf.len() {
        return Err(format!("ZIP: data out of bounds for {}", entry.name));
    }
    let compressed = &buf[data_start..data_end];

    match entry.method {
        0 => Ok(compressed.to_vec()),
        8 => {
            let mut decoder = DeflateDecoder::new(compressed);
            let mut out = Vec::with_capacity(entry.uncompressed_size as usize);
            decoder.read_to_end(&mut out).map_err(|e| e.to_string())?;
            Ok(out)
        }
        m => Err(format!("ZIP: unsupported method {} for {}", m, entry.name)),
    }
}

/// In-memory ZIP archive abstraction used for ORA import
pub struct ZipArchive {
    entries: Vec<ZipCentralEntry>,
    raw: Vec<u8>,
}

impl ZipArchive {
    pub fn read<R: Read + Seek>(mut reader: R) -> Result<Self, String> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        let entries = read_central_directory(&buf)?;
        Ok(Self { entries, raw: buf })
    }

    pub fn read_entry(&self, name: &str) -> Result<Vec<u8>, String> {
        for e in &self.entries {
            if e.name == name {
                return read_entry_data(&self.raw, e);
            }
        }
        Err(format!("ZIP: entry '{}' not found", name))
    }
}

/// Build a ZIP archive in memory with the given entries (name, method, data)
fn build_zip(entries: &[(String, u16, &[u8])]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut metas: Vec<ZipEntryMeta> = Vec::new();

    for (name, method, data) in entries {
        let offset = out.len() as u32;
        let mut entry = if *method == 8 {
            ZipEntryWriter::new_deflated(name.clone(), offset)
        } else {
            ZipEntryWriter::new_stored(name.clone(), offset)
        };
        entry.write(data).expect("zip write");
        let (compressed, meta) = entry.finalize().expect("zip finalize");
        write_local_header(&mut out, name, &meta);
        out.extend_from_slice(&compressed);
        metas.push(meta);
    }

    let central_offset = out.len() as u32;
    let mut central: Vec<u8> = Vec::new();
    for meta in &metas {
        write_central_header(&mut central, &meta.name, meta);
    }
    out.extend_from_slice(&central);
    let central_size = central.len() as u32;
    write_end_of_central(&mut out, metas.len() as u16, central_size, central_offset);
    out
}

/// Export the canvas to an OpenRaster (.ora) file
pub fn export_ora(
    path: &Path,
    layers: &AHashMap<u32, Layer>,
    layer_order: &[u32],
    canvas_width: u32,
    canvas_height: u32,
) -> Result<(), String> {
    let xml = build_stack_xml(layers, layer_order, canvas_width, canvas_height);

    let merged = flatten_composite(layers, layer_order, canvas_width, canvas_height);
    let merged_png = encode_png(canvas_width, canvas_height, &merged)?;

    let thumb_w = canvas_width.min(256);
    let thumb_h = canvas_height.min(256);
    let thumb = resize_rgba_nn(&merged, canvas_width, canvas_height, thumb_w, thumb_h);
    let thumb_png = encode_png(thumb_w, thumb_h, &thumb)?;

    let mut layer_pngs: Vec<Vec<u8>> = Vec::new();
    for &layer_id in layer_order.iter() {
        let layer = match layers.get(&layer_id) {
            Some(l) => l,
            None => continue,
        };
        let flat = flatten_layer_rgba(layer, canvas_width, canvas_height);
        let png_bytes = encode_png(canvas_width, canvas_height, &flat)?;
        layer_pngs.push(png_bytes);
    }

    // Build all (name, method, data) tuples first so the generic helper can reference them
    let mut entries: Vec<(String, u16, Vec<u8>)> = Vec::new();
    // mimetype MUST be stored uncompressed (method 0) per ORA spec
    entries.push(("mimetype".to_string(), 0u16, b"image/openraster".to_vec()));
    entries.push(("stack.xml".to_string(), 8u16, xml.into_bytes()));
    for (i, png_bytes) in layer_pngs.iter().enumerate() {
        entries.push((format!("data/layer{}.png", i), 8u16, png_bytes.clone()));
    }
    entries.push(("mergedimage.png".to_string(), 8u16, merged_png));
    entries.push(("Thumbnails/thumbnail.png".to_string(), 8u16, thumb_png));

    let refs: Vec<(String, u16, &[u8])> = entries
        .iter()
        .map(|(n, m, d)| (n.clone(), *m, d.as_slice()))
        .collect();
    let zip_bytes = build_zip(&refs);

    std::fs::write(path, &zip_bytes).map_err(|e| e.to_string())?;
    Ok(())
}

/// Imported layer data ready to be inserted into the app
pub struct ImportedLayer {
    pub name: String,
    pub opacity: f32,
    pub visible: bool,
    pub blend_mode: BlendMode,
    pub tiles: AHashMap<(i32, i32), Tile>,
}

/// Imported canvas data
pub struct ImportedCanvas {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<ImportedLayer>,
}

/// Import an OpenRaster (.ora) file
pub fn import_ora(path: &Path) -> Result<ImportedCanvas, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let archive = ZipArchive::read(file)?;

    // Verify mimetype
    if let Ok(mt) = archive.read_entry("mimetype") {
        let mt_str = String::from_utf8_lossy(&mt);
        if !mt_str.starts_with("image/openraster") {
            return Err(format!("Not an ORA file: mimetype = '{}'", mt_str));
        }
    }

    let xml_bytes = archive.read_entry("stack.xml")?;
    let xml = String::from_utf8(xml_bytes).map_err(|e| e.to_string())?;
    let stack = parse_stack_xml(&xml)?;

    let mut imported_layers = Vec::new();
    for entry in stack.layers.iter() {
        let layer_png = archive.read_entry(&entry.src)?;
        let (w, h, rgba) = decode_png(&layer_png)?;
        let tiles = rgba_to_tiles(&rgba, w, h);

        imported_layers.push(ImportedLayer {
            name: entry.name.clone(),
            opacity: entry.opacity,
            visible: entry.visible,
            blend_mode: entry.blend_mode,
            tiles,
        });
    }

    Ok(ImportedCanvas {
        width: stack.width,
        height: stack.height,
        layers: imported_layers,
    })
}

/// Apply an ImportedCanvas to the application state (replaces all layers)
pub fn apply_imported_canvas(
    imported: ImportedCanvas,
    layers: &mut AHashMap<u32, Layer>,
    layer_order: &mut Vec<u32>,
    layer_id_counter: &mut u32,
    active_layer_id: &mut u32,
) {
    layers.clear();
    layer_order.clear();

    for imp in imported.layers {
        *layer_id_counter += 1;
        let new_id = *layer_id_counter;
        let mut layer = Layer::new(new_id, imp.name);
        layer.opacity = imp.opacity;
        layer.visible = imp.visible;
        layer.blend_mode = imp.blend_mode;
        layer.kind = LayerType::Raster;
        layer.tiles = imp.tiles;
        layers.insert(new_id, layer);
        layer_order.push(new_id);
    }

    if let Some(&first) = layer_order.first() {
        *active_layer_id = first;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blend_mode_roundtrip() {
        for mode in [BlendMode::Normal, BlendMode::Multiply, BlendMode::Screen, BlendMode::Overlay, BlendMode::Luminosity, BlendMode::Shade] {
            let svg = blend_mode_to_svg(mode);
            let parsed = svg_to_blend_mode(svg);
            if matches!(mode, BlendMode::Normal | BlendMode::Luminosity | BlendMode::Shade) {
                assert_eq!(parsed, BlendMode::Normal);
            } else {
                assert_eq!(parsed, mode, "Roundtrip failed for {:?}", mode);
            }
        }
    }

    #[test]
    fn test_build_stack_xml() {
        let mut layers = AHashMap::new();
        let l1 = make_test_layer(1, "Layer 1", [255, 0, 0, 255]);
        let l2 = make_test_layer(2, "Layer 2", [0, 255, 0, 255]);
        layers.insert(1, l1);
        layers.insert(2, l2);
        let xml = build_stack_xml(&layers, &[1, 2], 100, 100);
        assert!(xml.contains("width=\"100\""));
        assert!(xml.contains("height=\"100\""));
        assert!(xml.contains("Layer 1"));
        assert!(xml.contains("Layer 2"));
        assert!(xml.contains("data/layer0.png"));
        assert!(xml.contains("data/layer1.png"));
    }

    #[test]
    fn test_parse_stack_xml() {
        let xml = "<?xml version=\"1.0\"?><image width=\"200\" height=\"150\"><stack><layer name=\"BG\" opacity=\"0.5\" visibility=\"visible\" blendmode=\"svg:multiply\" src=\"data/layer0.png\"/><layer name=\"FG\" opacity=\"1.0\" visibility=\"hidden\" blendmode=\"svg:screen\" src=\"data/layer1.png\"/></stack></image>";
        let stack = parse_stack_xml(xml).expect("parse");
        assert_eq!(stack.width, 200);
        assert_eq!(stack.height, 150);
        assert_eq!(stack.layers.len(), 2);
        assert_eq!(stack.layers[0].name, "BG");
        assert!((stack.layers[0].opacity - 0.5).abs() < 1e-6);
        assert!(stack.layers[0].visible);
        assert_eq!(stack.layers[0].blend_mode, BlendMode::Multiply);
        assert!(!stack.layers[1].visible);
        assert_eq!(stack.layers[1].blend_mode, BlendMode::Screen);
    }

    #[test]
    fn test_fix15_conversion() {
        let v = fix15_to_u8(32768);
        assert!(v >= 250, "fix15(32768) should be ~255, got {}", v);
        assert_eq!(fix15_to_u8(0), 0);
        let v = u8_to_fix15(255);
        assert!(v >= 32700, "u8(255) should be ~32768, got {}", v);
    }

    #[test]
    fn test_ora_export_import_roundtrip() {
        let mut layers = AHashMap::new();
        let mut layer = make_test_layer(1, "TestLayer", [200, 100, 50, 255]);
        for ly in 0..64 {
            for lx in 0..64 {
                let v = ((lx + ly) * 4) as u16;
                layer.tiles.get_mut(&(0, 0)).unwrap().pixels[ly][lx] = [v, 32768 - v, v / 2, 32768];
            }
        }
        layers.insert(1, layer);

        let layer_order = vec![1];
        let tmp = std::env::temp_dir().join("arty_ora_test.ora");
        export_ora(&tmp, &layers, &layer_order, 64, 64).expect("export");

        let imported = import_ora(&tmp).expect("import");
        assert_eq!(imported.width, 64);
        assert_eq!(imported.height, 64);
        assert_eq!(imported.layers.len(), 1);
        assert_eq!(imported.layers[0].name, "TestLayer");
        assert!(!imported.layers[0].tiles.is_empty(), "Should have tiles");

        let imp = &imported.layers[0];
        let tile = imp.tiles.get(&(0, 0)).expect("tile");
        let p = tile.pixels[10][10];
        let expected_r = fix15_to_u8(((10 + 10) * 4) as u16);
        assert!((fix15_to_u8(p[0]) as i32 - expected_r as i32).abs() <= 1, "R channel mismatch: got {}, expected {}", fix15_to_u8(p[0]), expected_r);
        assert!(p[3] > 30000, "Alpha should be ~opaque");

        let _ = std::fs::remove_file(&tmp);
    }

    fn make_test_layer(id: u32, name: &str, fill: [u8; 4]) -> Layer {
        let mut layer = Layer::new(id, name.to_string());
        layer.opacity = 1.0;
        layer.visible = true;
        layer.kind = LayerType::Raster;
        let mut tile = Tile::new();
        for ly in 0..64 {
            for lx in 0..64 {
                tile.pixels[ly][lx] = [fill[0] as u16 * 128, fill[1] as u16 * 128, fill[2] as u16 * 128, fill[3] as u16 * 128];
            }
        }
        layer.tiles.insert((0, 0), tile);
        layer
    }
}
