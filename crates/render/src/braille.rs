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
pub(crate) struct BrailleInstance {
    rect: [f32; 4],
    color: [f32; 4],
    geometry: [f32; 4],
}

impl BrailleInstance {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_pixels(
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        pattern: u8,
        color: [u8; 3],
        _viewport_width: f32,
        _viewport_height: f32,
    ) -> Self {
        let color = srgb_to_linear(color);
        Self {
            rect: [x, y, width, height],
            color: [color[0], color[1], color[2], 1.0],
            geometry: [width, height, f32::from(pattern), 0.0],
        }
    }
}

#[derive(Debug)]
pub(crate) struct BrailleRenderer {
    pipeline: RenderPipeline,
    rows: InstanceRows<BrailleInstance>,
}

impl BrailleRenderer {
    pub(crate) fn new(
        device: &wgpu::Device,
        format: TextureFormat,
        transform_layout: &BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Tmon Braille shader"),
            source: ShaderSource::Wgsl(Cow::Borrowed(include_str!("braille.wgsl"))),
        });
        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Tmon Braille pipeline layout"),
            bind_group_layouts: &[Some(transform_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Tmon Braille pipeline"),
            layout: Some(&layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &[Some(VertexBufferLayout {
                    array_stride: mem::size_of::<BrailleInstance>() as u64,
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
            rows: InstanceRows::new(4, "Tmon retained Braille row"),
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
        instances: &[BrailleInstance],
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
    fn one_cell_packs_all_eight_braille_dots_into_one_instance() {
        let instance = BrailleInstance::from_pixels(
            10.0,
            20.0,
            18.0,
            36.0,
            0xFF,
            [216, 222, 233],
            800.0,
            600.0,
        );

        assert_eq!(instance.geometry[2] as u8, 0xFF);
        assert!((instance.geometry[0] - 18.0).abs() < f32::EPSILON);
        assert!((instance.geometry[1] - 36.0).abs() < f32::EPSILON);
    }
}
