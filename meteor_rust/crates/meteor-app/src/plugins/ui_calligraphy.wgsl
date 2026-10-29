#import bevy_ui::ui_vertex_output::UiVertexOutput

@group(1) @binding(0) var title_texture: texture_2d<f32>;
@group(1) @binding(1) var title_sampler: sampler;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {

    let ink = textureSample(title_texture, title_sampler, in.uv).rgb;
    let coverage = max(ink.r, max(ink.g, ink.b));
    return vec4<f32>(ink / max(coverage, 0.00001), coverage);
}
