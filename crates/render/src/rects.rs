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
    instance_rows::{InstanceBuffer, InstanceRows, RowTransforms, UploadStats},
    palette::srgb_to_linear,
};

const ATTRIBUTES: [VertexAttribute; 2] = [
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
];

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct RectInstance {
    rect: [f32; 4],
    color: [f32; 4],
}

impl RectInstance {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_pixels(
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: [u8; 3],
        alpha: f32,
        _viewport_width: f32,
        _viewport_height: f32,
    ) -> Self {
        let color = srgb_to_linear(color);
        Self {
            rect: [x, y, width, height],
            color: [color[0], color[1], color[2], alpha],
        }
    }
}

#[derive(Debug)]
pub(crate) struct RectRenderer {
    pipeline: RenderPipeline,
    rows: InstanceRows<RectInstance>,
    overlay: InstanceBuffer<RectInstance>,
}

impl RectRenderer {
    pub(crate) fn new(
        device: &wgpu::Device,
        format: TextureFormat,
        transform_layout: &BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Tmon rectangle shader"),
            source: ShaderSource::Wgsl(Cow::Borrowed(include_str!("rects.wgsl"))),
        });
        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Tmon rectangle pipeline layout"),
            bind_group_layouts: &[Some(transform_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Tmon rectangle pipeline"),
            layout: Some(&layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &[Some(VertexBufferLayout {
                    array_stride: mem::size_of::<RectInstance>() as u64,
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
            rows: InstanceRows::new(8, "Tmon retained rectangle row"),
            overlay: InstanceBuffer::new(device, 8, "Tmon dynamic rectangle overlay"),
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
        instances: &[RectInstance],
    ) -> UploadStats {
        self.rows.upload(device, queue, row, instances)
    }

    pub(crate) fn upload_overlay(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[RectInstance],
    ) -> UploadStats {
        self.overlay.upload(device, queue, instances)
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
        if self.overlay.count() != 0 {
            pass.set_bind_group(0, transforms.bind_group(), &[transforms.overlay_offset()]);
            pass.set_vertex_buffer(0, self.overlay.buffer().slice(..));
            pass.draw(
                0..6,
                0..u32::try_from(self.overlay.count()).unwrap_or(u32::MAX),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RectInstance;

    #[test]
    fn rectangle_colors_are_linearized_for_the_srgb_surface() {
        let rectangle =
            RectInstance::from_pixels(0.0, 0.0, 10.0, 10.0, [11, 13, 15], 1.0, 100.0, 100.0);
        assert!(
            rectangle.color[0] < 0.01,
            "rectangle red channel was not converted to linear RGB"
        );
    }

    #[test]
    fn rectangle_geometry_stays_in_pixel_coordinates() {
        let rectangle =
            RectInstance::from_pixels(10.0, 20.0, 30.0, 40.0, [255; 3], 1.0, 100.0, 100.0);
        for (actual, expected) in rectangle.rect.into_iter().zip([10.0, 20.0, 30.0, 40.0]) {
            assert!((actual - expected).abs() < f32::EPSILON);
        }
    }
}
