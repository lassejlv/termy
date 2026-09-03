struct VertexInput {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) geometry: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) local_position: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) geometry: vec4<f32>,
};

@vertex
fn vertex_main(input: VertexInput, @builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let unit_corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
    );
    let origin = input.rect.xy + transform.offset;
    let extent = origin + input.rect.zw;
    let pixel_corners = array<vec2<f32>, 6>(
        vec2<f32>(origin.x, origin.y),
        vec2<f32>(origin.x, extent.y),
        vec2<f32>(extent.x, extent.y),
        vec2<f32>(origin.x, origin.y),
        vec2<f32>(extent.x, extent.y),
        vec2<f32>(extent.x, origin.y),
    );
    let pixel = pixel_corners[vertex_index];
    let clip = vec2<f32>(
        pixel.x / transform.viewport.x * 2.0 - 1.0,
        1.0 - pixel.y / transform.viewport.y * 2.0,
    );
    var output: VertexOutput;
    output.position = vec4<f32>(clip, 0.0, 1.0);
    output.local_position = unit_corners[vertex_index] * input.geometry.xy;
    output.color = input.color;
    output.geometry = input.geometry;
    return output;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let size = input.geometry.xy;
    let pattern = u32(round(input.geometry.z));
    let masks = array<u32, 8>(1u, 2u, 4u, 64u, 8u, 16u, 32u, 128u);
    let centers = array<vec2<f32>, 8>(
        vec2<f32>(0.34, 0.16),
        vec2<f32>(0.34, 0.39),
        vec2<f32>(0.34, 0.62),
        vec2<f32>(0.34, 0.85),
        vec2<f32>(0.76, 0.16),
        vec2<f32>(0.76, 0.39),
        vec2<f32>(0.76, 0.62),
        vec2<f32>(0.76, 0.85),
    );
    var distance = 1000000.0;
    for (var index = 0u; index < 8u; index += 1u) {
        if (pattern & masks[index]) != 0u {
            distance = min(distance, length(input.local_position - centers[index] * size));
        }
    }
    let radius = max(min(size.x * 0.12, size.y * 0.06), 0.75);
    let antialias = max(fwidth(distance) * 0.5, 0.25);
    let coverage = 1.0 - smoothstep(radius - antialias, radius + antialias, distance);
    return vec4<f32>(input.color.rgb, input.color.a * coverage);
}
struct Transform {
    viewport: vec2<f32>,
    offset: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> transform: Transform;
