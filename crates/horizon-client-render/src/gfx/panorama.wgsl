// CubeMap's position-only cube and panorama.vsh/fsh: sample its untransformed direction.
override HDR_WRITE_LIGHT: bool = false;
struct Projection { view_proj: mat4x4<f32> }
@group(0) @binding(0) var<uniform> projection: Projection;
@group(0) @binding(1) var panorama: texture_cube<f32>;
@group(0) @binding(2) var panorama_sampler: sampler;
struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) direction: vec3<f32>,
}
// The cube's 36 corner indices, 3 bits each and 10 to a word:
// 4 7 6 4 6 5 5 6 2 5 | 2 1 1 2 3 1 3 0 0 3 | 7 0 7 4 7 3 2 7 2 6 | 0 4 5 0 5 1.
// Constant arrays indexed by the vertex index make Adreno's shader compiler
// fail and lose the device, so the table and the corners are arithmetic.
fn cube_corner(index: u32) -> u32 {
    let word = select(
        select(select(0x2ad6e9bcu, 0x180cb44au, index >= 10u), 0x32e9f9c7u, index >= 20u),
        0xd160u,
        index >= 30u,
    );
    return (word >> (3u * (index % 10u))) & 7u;
}
@vertex fn vs_panorama(@builtin(vertex_index) index: u32) -> Out {
    // Corners 0-3 go round the z = -1 face from (-1, -1), 4-7 the z = 1 face.
    let c = cube_corner(index);
    let bits = vec3((c ^ (c >> 1u)) & 1u, (c >> 1u) & 1u, (c >> 2u) & 1u);
    let position = vec3<f32>(bits) * 2.0 - 1.0;
    var out: Out;
    out.clip = projection.view_proj * vec4(position, 1.0);
    out.direction = position;
    return out;
}
@fragment fn fs_panorama(in: Out) -> @location(0) vec4<f32> {
    let color = textureSample(panorama, panorama_sampler, in.direction);
    return vec4(color.rgb, select(color.a, 1.0, HDR_WRITE_LIGHT));
}
