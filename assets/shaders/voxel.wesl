// Deepcore voxel material: extends Bevy's StandardMaterial so that greedy-merged quads still
// show one full texture tile per block, instead of stretching a single texel across the quad.
//
// `uv`     (ATTRIBUTE_UV_0) carries the position inside one atlas tile, in 0..1.
// `uv_b`   (ATTRIBUTE_UV_1) carries how many tiles the quad spans in each direction.
// `color`  (ATTRIBUTE_COLOR) carries the atlas tile index in .r (flat interpolated).
//
// The fragment stage rebuilds the tile origin from the tile index, repeats the tile across
// the quad with `fract`, and clamps to just inside the tile border so bilinear filtering can
// never bleed neighbouring tiles into the result.

// Bevy 0.20 shaders are WESL modules. Bevy's own libraries are embedded under the
// `bevy_pbr` package, so a shader loaded from disk imports them by that package path.
import bevy_pbr::render::{
    pbr_bindings::{base_color_sampler, base_color_texture},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
};

@if(PREPASS_PIPELINE)
import bevy_pbr::{
    prepass::io::{VertexOutput, FragmentOutput},
    deferred::functions::deferred_output,
};
@else
import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
};

struct DeepcoreVoxelMaterial {
    // Atlas size in tiles, so the shader can convert a tile index into a UV origin.
    atlas_columns: f32,
    atlas_rows: f32,
    // Half a texel of inset, to keep bilinear filtering inside the tile.
    uv_inset: f32,
    // Strength of the emissive lift for glowing blocks.
    glow_strength: f32,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100)
var<uniform> voxel: DeepcoreVoxelMaterial;

// Repeats one atlas tile across a merged quad.
fn tile_uv(uv: vec2<f32>, uv_b: vec2<f32>, tile_index: f32) -> vec2<f32> {
    let columns = max(voxel.atlas_columns, 1.0);
    let rows = max(voxel.atlas_rows, 1.0);
    let tile = floor(tile_index + 0.5);
    let tile_origin = vec2<f32>(tile % columns, floor(tile / columns)) / vec2<f32>(columns, rows);
    let tile_size = vec2<f32>(1.0 / columns, 1.0 / rows);

    // How many tiles the quad spans; merged faces repeat the tile once per block.
    let repeat = max(uv_b, vec2<f32>(1.0));
    // Position inside the whole quad, in tile units, then wrapped into a single tile.
    let inside = fract(uv * repeat);
    // Keep the sample half a texel inside the tile so filtering never crosses the border.
    let inset = vec2<f32>(voxel.uv_inset);
    let clamped = clamp(inside, inset, vec2<f32>(1.0) - inset);
    return tile_origin + clamped * tile_size;
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    // Rebuild this fragment's atlas coordinate:
    //   * `in.uv`  = position inside the merged quad, in tile units (0..repeat)
    //   * `in.uv_b`= how many tiles the quad spans
    //   * `in.color.r` = atlas tile index
    let columns = max(voxel.atlas_columns, 1.0);
    let rows = max(voxel.atlas_rows, 1.0);
    let tile_size = vec2<f32>(1.0 / columns, 1.0 / rows);
    let tile_index = floor(in.color.r + 0.5);
    let tile_origin = vec2<f32>(tile_index % columns, floor(tile_index / columns)) * tile_size;

    // Repeat the tile once per block, then clamp just inside its border so bilinear
    // filtering can never bleed a neighbouring tile into the result.
    let repeat = max(in.uv_b, vec2<f32>(1.0));
    let inset = vec2<f32>(voxel.uv_inset);
    let wrapped = clamp(fract(in.uv * repeat), inset, vec2<f32>(1.0) - inset);
    let atlas_uv = tile_origin + wrapped * tile_size;

    // Sample the atlas ourselves, then cancel the standard material's own base-colour
    // texture multiply by dividing the material colour out before it is applied.
    // Sample the atlas at the *tiled* coordinate rather than the raw interpolated UV.
    let sampled = textureSample(base_color_texture, base_color_sampler, atlas_uv).rgb;

    var pbr_input = pbr_input_from_standard_material(in, is_front);
    // Replace the base colour the standard material computed at the raw UV with our tiled
    // sample, preserving the material tint (which is white for the voxel material).
    pbr_input.material.base_color = vec4<f32>(sampled, 1.0);

    // Emissive lift for glowing blocks (uranium, lamps, molten rock).
    let emissive = voxel.glow_strength;
    if (emissive > 0.0) {
        pbr_input.material.emissive = pbr_input.material.emissive + vec4<f32>(sampled * emissive, 0.0);
    }

    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color,
    );

    var out: FragmentOutput;
@if(PREPASS_PIPELINE) {
    out = deferred_output(in, pbr_input);
} @else {
    var color = apply_pbr_lighting(pbr_input);
    color = main_pass_post_lighting_processing(pbr_input, color);
    out.color = color;
}
    return out;
}
