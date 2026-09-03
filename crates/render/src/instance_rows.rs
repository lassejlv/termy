use std::{marker::PhantomData, mem, num::NonZeroU64};

use bytemuck::{Pod, Zeroable};
use engine::{RowMove, RowMoveDirection};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BufferBinding, BufferBindingType,
    BufferDescriptor, BufferSize, BufferUsages, Device, Queue, ShaderStages,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct UploadStats {
    pub writes: u64,
    pub bytes: u64,
    pub growths: u64,
}

#[derive(Debug)]
pub(crate) struct InstanceBuffer<T> {
    buffer: wgpu::Buffer,
    capacity: usize,
    count: usize,
    label: &'static str,
    marker: PhantomData<T>,
}

impl<T: Pod> InstanceBuffer<T> {
    pub(crate) fn new(device: &Device, capacity: usize, label: &'static str) -> Self {
        let capacity = capacity.max(1);
        Self {
            buffer: make_instance_buffer::<T>(device, capacity, label),
            capacity,
            count: 0,
            label,
            marker: PhantomData,
        }
    }

    pub(crate) fn upload(
        &mut self,
        device: &Device,
        queue: &Queue,
        instances: &[T],
    ) -> UploadStats {
        let mut stats = UploadStats::default();
        if instances.len() > self.capacity {
            self.capacity = instances.len().next_power_of_two();
            self.buffer = make_instance_buffer::<T>(device, self.capacity, self.label);
            stats.growths = 1;
        }
        self.count = instances.len();
        if !instances.is_empty() {
            let bytes = bytemuck::cast_slice(instances);
            queue.write_buffer(&self.buffer, 0, bytes);
            stats.writes = 1;
            stats.bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        }
        stats
    }

    pub(crate) const fn count(&self) -> usize {
        self.count
    }

    pub(crate) const fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }
}

#[derive(Debug)]
pub(crate) struct InstanceRows<T> {
    rows: Vec<InstanceBuffer<T>>,
    initial_capacity: usize,
    label: &'static str,
}

impl<T: Pod> InstanceRows<T> {
    pub(crate) const fn new(initial_capacity: usize, label: &'static str) -> Self {
        Self {
            rows: Vec::new(),
            initial_capacity,
            label,
        }
    }

    pub(crate) fn resize(&mut self, device: &Device, rows: usize) {
        self.rows.truncate(rows);
        while self.rows.len() < rows {
            self.rows.push(InstanceBuffer::new(
                device,
                self.initial_capacity,
                self.label,
            ));
        }
    }

    pub(crate) fn move_rows(&mut self, movement: RowMove) -> bool {
        let height = movement.end_row.saturating_sub(movement.start_row);
        if movement.start_row >= movement.end_row
            || movement.end_row > self.rows.len()
            || movement.count == 0
            || movement.count >= height
        {
            return false;
        }
        let rows = &mut self.rows[movement.start_row..movement.end_row];
        match movement.direction {
            RowMoveDirection::Up => rows.rotate_left(movement.count),
            RowMoveDirection::Down => rows.rotate_right(movement.count),
        }
        true
    }

    pub(crate) fn upload(
        &mut self,
        device: &Device,
        queue: &Queue,
        row: usize,
        instances: &[T],
    ) -> UploadStats {
        self.rows
            .get_mut(row)
            .map_or_else(UploadStats::default, |buffer| {
                buffer.upload(device, queue, instances)
            })
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &InstanceBuffer<T>> {
        self.rows.iter()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct TransformUniform {
    viewport: [f32; 2],
    offset: [f32; 2],
}

#[derive(Debug)]
pub(crate) struct RowTransforms {
    buffer: wgpu::Buffer,
    bind_group: BindGroup,
    stride: usize,
    rows: usize,
    scratch: Vec<u8>,
}

impl RowTransforms {
    pub(crate) fn layout(device: &Device) -> BindGroupLayout {
        device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Tmon instance row transform layout"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: BufferSize::new(mem::size_of::<TransformUniform>() as u64),
                },
                count: None,
            }],
        })
    }

    pub(crate) fn new(device: &Device, layout: &BindGroupLayout, rows: usize) -> Self {
        let stride = align_up(
            mem::size_of::<TransformUniform>(),
            device.limits().min_uniform_buffer_offset_alignment as usize,
        );
        let (buffer, bind_group) = make_transform_buffer(device, layout, stride, rows);
        Self {
            buffer,
            bind_group,
            stride,
            rows,
            scratch: Vec::new(),
        }
    }

    pub(crate) fn resize(
        &mut self,
        device: &Device,
        layout: &BindGroupLayout,
        rows: usize,
    ) -> bool {
        if rows == self.rows {
            return false;
        }
        let (buffer, bind_group) = make_transform_buffer(device, layout, self.stride, rows);
        self.buffer = buffer;
        self.bind_group = bind_group;
        self.rows = rows;
        true
    }

    pub(crate) fn update(
        &mut self,
        queue: &Queue,
        viewport_width: f32,
        viewport_height: f32,
        padding: f32,
        row_height: f32,
    ) -> u64 {
        let entry_count = self.rows.saturating_add(1);
        self.scratch.clear();
        self.scratch
            .resize(self.stride.saturating_mul(entry_count), 0);
        for row in 0..self.rows {
            self.write_entry(
                row,
                TransformUniform {
                    viewport: [viewport_width, viewport_height],
                    offset: [padding, padding + row as f32 * row_height],
                },
            );
        }
        self.write_entry(
            self.rows,
            TransformUniform {
                viewport: [viewport_width, viewport_height],
                offset: [0.0, 0.0],
            },
        );
        queue.write_buffer(&self.buffer, 0, &self.scratch);
        u64::try_from(self.scratch.len()).unwrap_or(u64::MAX)
    }

    pub(crate) const fn bind_group(&self) -> &BindGroup {
        &self.bind_group
    }

    pub(crate) fn row_offset(&self, row: usize) -> u32 {
        u32::try_from(row.saturating_mul(self.stride)).unwrap_or(u32::MAX)
    }

    pub(crate) fn overlay_offset(&self) -> u32 {
        self.row_offset(self.rows)
    }

    fn write_entry(&mut self, index: usize, uniform: TransformUniform) {
        let start = index.saturating_mul(self.stride);
        let bytes = bytemuck::bytes_of(&uniform);
        self.scratch[start..start + bytes.len()].copy_from_slice(bytes);
    }
}

fn make_instance_buffer<T>(device: &Device, capacity: usize, label: &'static str) -> wgpu::Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: capacity.saturating_mul(mem::size_of::<T>()) as u64,
        usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn make_transform_buffer(
    device: &Device,
    layout: &BindGroupLayout,
    stride: usize,
    rows: usize,
) -> (wgpu::Buffer, BindGroup) {
    let size = stride.saturating_mul(rows.saturating_add(1)).max(stride) as u64;
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("Tmon instance row transforms"),
        size,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some("Tmon instance row transform bind group"),
        layout,
        entries: &[BindGroupEntry {
            binding: 0,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: &buffer,
                offset: 0,
                size: NonZeroU64::new(mem::size_of::<TransformUniform>() as u64),
            }),
        }],
    });
    (buffer, bind_group)
}

fn align_up(value: usize, alignment: usize) -> usize {
    value
        .div_ceil(alignment.max(1))
        .saturating_mul(alignment.max(1))
}
