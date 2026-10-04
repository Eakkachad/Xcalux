// Draws the composited page: one instanced quad per texture-array chunk.

struct Uniforms {
    // doc px -> clip space, rows of a 2x3 affine
    row0: vec4<f32>,
    row1: vec4<f32>,
    // page width, page height, chunk size, chunks per row
    page: vec4<f32>,
    // checker cell (doc px), target is sRGB (1/0), _, _
    misc: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var page_tex: texture_2d_array<f32>;
@group(0) @binding(2) var page_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) doc: vec2<f32>,
    @location(2) @interpolate(flat) layer: u32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let per_row = u32(u.page.w);
    let cs = u.page.z;
    let origin = vec2<f32>(f32(ii % per_row), f32(ii / per_row)) * cs;
    let far = min(origin + vec2<f32>(cs, cs), u.page.xy);

    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let k = corners[vi];
    let p = mix(origin, far, k);

    var out: VsOut;
    let h = vec3<f32>(p, 1.0);
    out.pos = vec4<f32>(dot(u.row0.xyz, h), dot(u.row1.xyz, h), 0.0, 1.0);
    out.uv = (p - origin) / cs;
    out.doc = p;
    out.layer = ii;
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Premultiplied, display-space (sRGB-encoded) color.
    let c = textureSample(page_tex, page_sampler, in.uv, in.layer);
    let cell = floor(in.doc / u.misc.x);
    let odd = (i32(cell.x) + i32(cell.y)) & 1;
    let bg = select(vec3<f32>(1.0), vec3<f32>(0.86), odd == 1);
    var rgb = c.rgb + bg * (1.0 - c.a);
    if (u.misc.y > 0.5) {
        rgb = srgb_to_linear(rgb);
    }
    return vec4<f32>(rgb, 1.0);
}
