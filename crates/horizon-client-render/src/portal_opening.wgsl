// The portal view's opening (gfx.rs, `Frame::portal`): the entry portal's rectangle, drawn in the
// world pass with the scene's depth, showing the level behind it as the portal pass drew it
// into a target the size of the world's.
struct Opening {
    // The opening's corners in clip space, in order around it.
    corners: array<vec4<f32>, 4>,
}
@group(0) @binding(0) var<uniform> opening: Opening;
@group(0) @binding(1) var behind: texture_2d<f32>;

@vertex fn vs_opening(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var order = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    return opening.corners[order[i]];
}

@fragment fn fs_opening(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(behind)) - 1;
    return textureLoad(behind, clamp(vec2<i32>(at.xy), vec2(0), size), 0);
}
