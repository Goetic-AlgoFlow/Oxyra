// The glow layer (`text_glow.rs`): mods' glowing text, drawn alone into a
// cleared texture, is halved, blurred and added to the frame (bloom).
struct Pass {
    // Source texel size (1/width, 1/height), blur direction in texels.
    texel_dir: vec4<f32>,
    // Strength of the final add, unused, unused, unused.
    strength: vec4<f32>,
}
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> pass_: Pass;
struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> Out {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: Out;
    out.clip = vec4(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2(p.x, 1.0 - p.y);
    return out;
}
// Half size: each pixel's centre is the shared corner of its 2x2 block, so one
// bilinear sample is their mean.
@fragment fn fs_down(in: Out) -> @location(0) vec4<f32> {
    return textureSampleLevel(src, samp, in.uv, 0.0);
}
// A 9-tap Gaussian in one direction, in 5 bilinear samples.
@fragment fn fs_blur(in: Out) -> @location(0) vec4<f32> {
    let d = pass_.texel_dir.xy * pass_.texel_dir.zw;
    var c = textureSampleLevel(src, samp, in.uv, 0.0) * 0.2270270;
    c += textureSampleLevel(src, samp, in.uv + d * 1.3846154, 0.0) * 0.3162162;
    c += textureSampleLevel(src, samp, in.uv - d * 1.3846154, 0.0) * 0.3162162;
    c += textureSampleLevel(src, samp, in.uv + d * 3.2307692, 0.0) * 0.0702703;
    c += textureSampleLevel(src, samp, in.uv - d * 3.2307692, 0.0) * 0.0702703;
    return c;
}
// Laid over the frame, premultiplied (blend One, OneMinusSrcAlpha; the frame's
// alpha kept): a glyph drawn over cleared black left colour × alpha. Over a dark
// sky it adds light, over a bright one it tints instead of turning white.
@fragment fn fs_add(in: Out) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(src, samp, in.uv, 0.0) * pass_.strength.x;
    let a = min(c.a, 1.0);
    return vec4(min(c.rgb, vec3(a)), a);
}
