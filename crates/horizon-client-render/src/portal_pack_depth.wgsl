// A shader pack's depth into the world's depth target (gfx.rs, `portal_openings_over`): the game's
// passes never drew the pack's terrain there, and the openings are tested against it. Pack buffers
// keep GL's rows and window depth: reverse-Z depth is 1 - d for the same near and far planes.
@group(0) @binding(0) var pack_depth: texture_depth_2d;

@vertex fn vs_depth(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let at = vec2<f32>(f32(i & 1u), f32(i >> 1u)) * 4.0 - 1.0;
    return vec4<f32>(at, 0.0, 1.0);
}

@fragment fn fs_depth(@builtin(position) p: vec4<f32>) -> @builtin(frag_depth) f32 {
    let size = vec2<i32>(textureDimensions(pack_depth));
    return 1.0 - textureLoad(pack_depth, vec2<i32>(i32(p.x), size.y - 1 - i32(p.y)), 0);
}
