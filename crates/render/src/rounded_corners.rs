use std::{borrow::Cow, mem};

use bytemuck::{Pod, Zeroable};
use engine::RowMove;
use wgpu::{
    BindGroupLayout, BlendState, ColorTargetState, ColorWrites, FragmentState, MultisampleState,
    PipelineCompilationOptions, PipelineLayoutDescriptor, PrimitiveState, RenderPass,
    RenderPipeline, RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource, TextureFormat,
    VertexAttribute, VertexBufferLayout, VertexFormat, VertexState, VertexStepMode,
};

use crate::{
    box_drawing::RoundedCorner,
    instance_rows::{InstanceRows, RowTransforms, UploadStats},
    palette::srgb_to_linear,
};

const ATTRIBUTES: [VertexAttribute; 3] = [
    VertexAttribute {
        format: VertexFormat::Float32x4,
        offset: 0,
        shader_location: 0,
    },
    VertexAttribute {
        format: VertexFormat::Float32x4,
        offset: 16,
        shader_location: 1,
    },
    VertexAttribute {
        format: VertexFormat::Float32x4,
        offset: 32,
        shader_location: 2,
    },
];

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct RoundedCornerInstance {
    rect: [f32; 4],
    color: [f32; 4],
    geometry: [f32; 4],
}

impl RoundedCornerInstance {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_pixels(
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        font_size: f32,
        corner: RoundedCorner,
        color: [u8; 3],
        _viewport_width: f32,
        _viewport_height: f32,
    ) -> Self {
        let color = srgb_to_linear(color);
        let stroke = (font_size * 0.0675).ceil().max(1.0);
        Self {
            rect: [x, y, width, height],
            color: [color[0], color[1], color[2], 1.0],
            geometry: [width, height, stroke, corner.shader_orientation()],
        }
    }
}

#[derive(Debug)]
pub(crate) struct RoundedCornerRenderer {
    pipeline: RenderPipeline,
    rows: InstanceRows<RoundedCornerInstance>,
}

impl RoundedCornerRenderer {
    pub(crate) fn new(
        device: &wgpu::Device,
        format: TextureFormat,
        transform_layout: &BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Tmon rounded-corner shader"),
            source: ShaderSource::Wgsl(Cow::Borrowed(include_str!("rounded_corners.wgsl"))),
        });
        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Tmon rounded-corner pipeline layout"),
            bind_group_layouts: &[Some(transform_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Tmon rounded-corner pipeline"),
            layout: Some(&layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &[Some(VertexBufferLayout {
                    array_stride: mem::size_of::<RoundedCornerInstance>() as u64,
                    step_mode: VertexStepMode::Instance,
                    attributes: &ATTRIBUTES,
                })],
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fragment_main"),
                compilation_options: PipelineCompilationOptions::default(),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(BlendState::ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            rows: InstanceRows::new(2, "Tmon retained rounded-corner row"),
        }
    }

    pub(crate) fn resize_rows(&mut self, device: &wgpu::Device, rows: usize) {
        self.rows.resize(device, rows);
    }

    pub(crate) fn move_rows(&mut self, movement: RowMove) -> bool {
        self.rows.move_rows(movement)
    }

    pub(crate) fn upload_row(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        row: usize,
        instances: &[RoundedCornerInstance],
    ) -> UploadStats {
        self.rows.upload(device, queue, row, instances)
    }

    pub(crate) fn draw<'pass>(
        &'pass self,
        pass: &mut RenderPass<'pass>,
        transforms: &'pass RowTransforms,
    ) {
        pass.set_pipeline(&self.pipeline);
        for (row, buffer) in self.rows.iter().enumerate() {
            if buffer.count() == 0 {
                continue;
            }
            pass.set_bind_group(0, transforms.bind_group(), &[transforms.row_offset(row)]);
            pass.set_vertex_buffer(0, buffer.buffer().slice(..));
            pass.draw(0..6, 0..u32::try_from(buffer.count()).unwrap_or(u32::MAX));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_geometry_uses_the_same_light_stroke_as_box_lines() {
        let corner = RoundedCornerInstance::from_pixels(
            10.0,
            20.0,
            18.0,
            36.0,
            30.0,
            RoundedCorner::UpperLeft,
            [216, 222, 233],
            800.0,
            600.0,
        );

        for (actual, expected) in corner.geometry.into_iter().zip([18.0, 36.0, 3.0, 0.0]) {
            assert!((actual - expected).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn each_corner_has_a_distinct_shader_orientation() {
        for (corner, expected) in [
            (RoundedCorner::UpperLeft, 0.0),
            (RoundedCorner::UpperRight, 1.0),
            (RoundedCorner::LowerRight, 2.0),
            (RoundedCorner::LowerLeft, 3.0),
        ] {
            assert!((corner.shader_orientation() - expected).abs() < f32::EPSILON);
        }
    }
}
