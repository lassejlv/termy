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

fn segment_distance(point: vec2<f32>, start: vec2<f32>, end: vec2<f32>) -> f32 {
    let segment = end - start;
    let fraction = clamp(
        dot(point - start, segment) / max(dot(segment, segment), 0.0001),
        0.0,
        1.0,
    );
    return length(point - (start + segment * fraction));
}

fn upper_left_arc_distance(point: vec2<f32>, center: vec2<f32>, radius: f32) -> f32 {
    let relative = point - center;
    let quadrant = min(relative, vec2<f32>(0.0, 0.0));
    let quadrant_length = length(quadrant);
    if quadrant_length > 0.0001 {
        let closest = center + quadrant / quadrant_length * radius;
        return length(point - closest);
    }
    let left_end = center + vec2<f32>(-radius, 0.0);
    let top_end = center + vec2<f32>(0.0, -radius);
    return min(length(point - left_end), length(point - top_end));
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let width = input.geometry.x;
    let height = input.geometry.y;
    let stroke = input.geometry.z;
    let orientation = input.geometry.w;
    var point = input.local_position;

    // Normalize all four glyphs to the upper-left corner (╭).
    if orientation > 0.5 && orientation < 2.5 {
        point.x = width - point.x;
    }
    if orientation > 1.5 {
        point.y = height - point.y;
    }

    // Match the device-pixel snapping used by the rectilinear box strokes.
    let center_x = floor(max(width - stroke, 0.0) * 0.5 + 0.5) + stroke * 0.5;
    let center_y = floor(max(height - stroke, 0.0) * 0.5 + 0.5) + stroke * 0.5;
    let radius = max((min(width, height) - stroke) * 0.5, 0.0);
    let arc_center = vec2<f32>(center_x + radius, center_y + radius);
    let vertical_distance = segment_distance(
        point,
        vec2<f32>(center_x, center_y + radius),
        vec2<f32>(center_x, height),
    );
    let horizontal_distance = segment_distance(
        point,
        vec2<f32>(center_x + radius, center_y),
        vec2<f32>(width, center_y),
    );
    let arc_distance = upper_left_arc_distance(point, arc_center, radius);
    let distance = min(min(vertical_distance, horizontal_distance), arc_distance);
    let antialias = max(fwidth(distance) * 0.5, 0.25);
    let coverage = 1.0 - smoothstep(stroke * 0.5 - antialias, stroke * 0.5 + antialias, distance);
    return vec4<f32>(input.color.rgb, input.color.a * coverage);
}
struct Transform {
    viewport: vec2<f32>,
    offset: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> transform: Transform;
