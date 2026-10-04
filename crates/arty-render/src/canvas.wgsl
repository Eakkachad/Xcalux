// Draws the composited page as one quad. The page is stored as square chunks
// in a texture array; filtering is done by hand with textureLoad so taps
// cross chunk layers without seams and clamp at the page edge (texels past
// the page inside the last chunk are never written).

struct Uniforms {
    // doc px -> clip space, rows of a 2x3 affine
    row0: vec4<f32>,
    row1: vec4<f32>,
    // page width, page height, chunk size, chunks per row
    page: vec4<f32>,
    // checker cell (doc px), target is sRGB (1/0), mip lod, _
    misc: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var page_tex: texture_2d_array<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) doc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let p = corners[vi] * u.page.xy;

    var out: VsOut;
    let h = vec3<f32>(p, 1.0);
    out.pos = vec4<f32>(dot(u.row0.xyz, h), dot(u.row1.xyz, h), 0.0, 1.0);
    out.doc = p;
    return out;
}

// Page texel `t` of mip `lvl`; same mapping as `chunk_slot` in gpu.rs with
// `side = chunk >> lvl`.
fn fetch(t: vec2<i32>, side: i32, lvl: i32) -> vec4<f32> {
    let layer = (t.y / side) * i32(u.page.w) + t.x / side;
    return textureLoad(page_tex, t % side, layer, lvl);
}

// Bilinear sample of mip `lvl` at doc position `doc`, clamped to the texels
// overlapping the page (`level_extent` in gpu.rs).
fn sample_level(doc: vec2<f32>, lvl: i32) -> vec4<f32> {
    let scale = exp2(f32(lvl));
    let side = i32(u.page.z) >> u32(lvl);
    let hi = vec2<i32>(ceil(u.page.xy / scale)) - vec2<i32>(1);
    let t = doc / scale - 0.5;
    let base = floor(t);
    let f = t - base;
    let i0 = clamp(vec2<i32>(base), vec2<i32>(0), hi);
    let i1 = clamp(vec2<i32>(base) + vec2<i32>(1), vec2<i32>(0), hi);
    let top = mix(fetch(i0, side, lvl), fetch(vec2<i32>(i1.x, i0.y), side, lvl), f.x);
    let bottom = mix(fetch(vec2<i32>(i0.x, i1.y), side, lvl), fetch(i1, side, lvl), f.x);
    return mix(top, bottom, f.y);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Premultiplied, display-space (sRGB-encoded) color; trilinear between
    // the two mips around the LOD.
    let lod = u.misc.z;
    let l0 = floor(lod);
    var c = sample_level(in.doc, i32(l0));
    if (lod > l0) {
        c = mix(c, sample_level(in.doc, i32(l0) + 1), lod - l0);
    }
    let cell = floor(in.doc / u.misc.x);
    let odd = (i32(cell.x) + i32(cell.y)) & 1;
    let bg = select(vec3<f32>(1.0), vec3<f32>(0.86), odd == 1);
    var rgb = c.rgb + bg * (1.0 - c.a);
    if (u.misc.y > 0.5) {
        rgb = srgb_to_linear(rgb);
    }
    return vec4<f32>(rgb, 1.0);
}
