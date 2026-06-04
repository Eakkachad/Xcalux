struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coords: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

@vertex
fn vs_main(model: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(model.position, 0.0, 1.0);
    out.tex_coords = model.tex_coords;
    return out;
}

// Bindings for layer composition
@group(0) @binding(0) var t_background: texture_2d<f32>;
@group(0) @binding(1) var s_background: sampler;
@group(0) @binding(2) var t_foreground: texture_2d<f32>;
@group(0) @binding(3) var s_foreground: sampler;
@group(0) @binding(4) var t_mask: texture_2d<f32>;

struct BlendUniforms {
    opacity: f32,
    blend_mode: u32, // 0: Normal, 1: Multiply, 2: Screen, 3: Overlay, 4: Luminosity, 5: Shade
    clipping: u32,   // 0: Disabled, 1: Enabled
    padding: u32,
};

@group(1) @binding(0) var<uniform> uniforms: BlendUniforms;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    if (uniforms.blend_mode == 6u) {
        return vec4<f32>(1.0, 1.0, 1.0, 1.0);
    }

    let bg_dimensions = vec2<f32>(textureDimensions(t_background));
    let bg_tex_coords = in.clip_position.xy / bg_dimensions;
    let dst = textureSample(t_background, s_background, bg_tex_coords);
    let src_raw = textureSample(t_foreground, s_foreground, in.tex_coords);
    let mask_val = textureSample(t_mask, s_foreground, in.tex_coords).r;
    
    // The CPU stored tiles are premultiplied fix15, but we upload them to the GPU
    // as Rgba8Unorm, which has premultiplied sRGB colors.
    // Let's get the straight alpha source color first to apply opacity and blend modes properly.
    let src_alpha = src_raw.a * uniforms.opacity * mask_val;
    if (src_alpha <= 0.0) {
        return dst;
    }
    
    // Un-premultiply foreground for standard algebraic color blending
    var src_rgb = vec3<f32>(0.0);
    if (src_raw.a > 0.0) {
        src_rgb = src_raw.rgb / src_raw.a;
    } else {
        src_rgb = src_raw.rgb;
    }
    
    var blend_rgb = vec3<f32>(0.0);
    
    switch (uniforms.blend_mode) {
        case 0u: { // Normal
            blend_rgb = src_rgb;
        }
        case 1u: { // Multiply
            blend_rgb = dst.rgb * src_rgb;
        }
        case 2u: { // Screen
            blend_rgb = 1.0 - (1.0 - dst.rgb) * (1.0 - src_rgb);
        }
        case 3u: { // Overlay
            var r = 0.0;
            var g = 0.0;
            var b = 0.0;
            
            if (dst.r < 0.5) { r = 2.0 * dst.r * src_rgb.r; } else { r = 1.0 - 2.0 * (1.0 - dst.r) * (1.0 - src_rgb.r); }
            if (dst.g < 0.5) { g = 2.0 * dst.g * src_rgb.g; } else { g = 1.0 - 2.0 * (1.0 - dst.g) * (1.0 - src_rgb.g); }
            if (dst.b < 0.5) { b = 2.0 * dst.b * src_rgb.b; } else { b = 1.0 - 2.0 * (1.0 - dst.b) * (1.0 - src_rgb.b); }
            
            blend_rgb = vec3<f32>(r, g, b);
        }
        case 4u: { // Luminosity (Shine)
            let shine_effect = dst.rgb + (src_rgb * src_alpha);
            return vec4<f32>(clamp(shine_effect, vec3<f32>(0.0), vec3<f32>(1.0)), max(dst.a, src_alpha));
        }
        case 5u: { // Shade
            let shade_effect = dst.rgb * (1.0 - src_rgb * src_alpha * 0.5);
            return vec4<f32>(clamp(shade_effect, vec3<f32>(0.0), vec3<f32>(1.0)), max(dst.a, src_alpha));
        }
        case 6u: { // Solid White Paper Canvas Sheet
            return vec4<f32>(1.0, 1.0, 1.0, 1.0);
        }
        default: {
            blend_rgb = src_rgb;
        }
    }
    
    // Clipping Group behavior: Multiply foreground's alpha by background's alpha
    let final_alpha = select(src_alpha, src_alpha * dst.a, uniforms.clipping == 1u);
    
    // Standard premultiplied alpha compositing
    let final_rgb = dst.rgb * (1.0 - final_alpha) + blend_rgb * final_alpha;
    let final_a = max(dst.a, final_alpha);
    
    return vec4<f32>(clamp(final_rgb, vec3<f32>(0.0), vec3<f32>(1.0)), final_a);
}
