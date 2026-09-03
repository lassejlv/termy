struct Transform {
    viewport: vec2<f32>,
    offset: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> transform: Transform;

struct VertexInput {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

fn to_clip(pixel: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        pixel.x / transform.viewport.x * 2.0 - 1.0,
        1.0 - pixel.y / transform.viewport.y * 2.0,
    );
}

@vertex
fn vertex_main(input: VertexInput, @builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let origin = input.rect.xy + transform.offset;
    let extent = origin + input.rect.zw;
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(origin.x, origin.y),
        vec2<f32>(origin.x, extent.y),
        vec2<f32>(extent.x, extent.y),
        vec2<f32>(origin.x, origin.y),
        vec2<f32>(extent.x, extent.y),
        vec2<f32>(extent.x, origin.y),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(to_clip(corners[vertex_index]), 0.0, 1.0);
    output.color = input.color;
    return output;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}
