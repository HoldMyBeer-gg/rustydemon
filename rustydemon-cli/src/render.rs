//! Headless wgpu mesh renderer — renders a mesh to a PNG file without
//! opening a window.  Used by `inspect --png` to produce a visual
//! preview from the CLI so automated agents can verify 3D output.

use std::path::Path;

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

/// Positions + UVs + indices + bounding box — the subset of mesh data
/// we need for rendering.  Kept separate from the GUI's `Mesh3dCpu` so
/// the CLI doesn't depend on the GUI crate.
pub struct RenderMesh {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct BatchUniform {
    color: [f32; 4],
}

const SHADER: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct BatchUniform {
    color: vec4<f32>,
};
@group(1) @binding(0) var<uniform> b: BatchUniform;

@group(2) @binding(0) var mat_tex: texture_2d<f32>;
@group(2) @binding(1) var mat_smp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
) -> VsOut {
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(pos, 1.0);
    out.world = pos;
    out.uv = uv;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dx = dpdx(in.world);
    let dy = dpdy(in.world);
    let n = normalize(cross(dx, dy));
    let light_dir = normalize(vec3<f32>(0.4, 0.8, 0.5));
    let lambert = max(dot(n, light_dir), 0.0);
    let shaded = 0.3 + 0.7 * lambert;
    let tex = textureSample(mat_tex, mat_smp, in.uv);
    return vec4<f32>(tex.rgb * b.color.rgb * shaded, 1.0);
}
"#;

/// Render `mesh` to a PNG file at `path` using a headless wgpu device.
/// Returns the pixel dimensions written.
pub fn render_to_png(mesh: &RenderMesh, path: &Path, width: u32, height: u32) -> Result<()> {
    pollster::block_on(render_to_png_async(mesh, path, width, height))
}

async fn render_to_png_async(
    mesh: &RenderMesh,
    path: &Path,
    width: u32,
    height: u32,
) -> Result<()> {
    // ── 1. Headless device ───────────────────────────────────────────────
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
        .context("no wgpu adapter available")?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default(), None)
        .await
        .context("failed to create wgpu device")?;

    let color_format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let depth_format = wgpu::TextureFormat::Depth32Float;

    // ── 2. Pipeline ──────────────────────────────────────────────────────
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("headless shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });

    let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("uniform bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });

    let batch_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("batch bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<BatchUniform>() as u64),
            },
            count: None,
        }],
    });

    let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("texture bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("headless pl"),
        bind_group_layouts: &[&uniform_bgl, &batch_bgl, &texture_bgl],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("headless pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_main",
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: (std::mem::size_of::<f32>() * 5) as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Cw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    // ── 3. Upload mesh ───────────────────────────────────────────────────
    let vert_count = mesh.positions.len();
    let mut flat: Vec<f32> = Vec::with_capacity(vert_count * 5);
    for i in 0..vert_count {
        let p = mesh.positions[i];
        let uv = mesh.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
        flat.extend_from_slice(&[p[0], p[1], p[2], uv[0], uv[1]]);
    }
    let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("vertex buf"),
        contents: bytemuck::cast_slice(&flat),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("index buf"),
        contents: bytemuck::cast_slice(&mesh.indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    // ── 4. View-projection ───────────────────────────────────────────────
    let mn = Vec3::from(mesh.bbox_min);
    let mx = Vec3::from(mesh.bbox_max);
    let center = (mn + mx) * 0.5;
    let extent = (mx - mn).length().max(1.0);

    let yaw: f32 = 0.6;
    let pitch: f32 = 0.45;
    let distance = extent * 1.4;
    let cos_p = pitch.cos();
    let eye = center
        + Vec3::new(
            distance * cos_p * yaw.cos(),
            distance * cos_p * yaw.sin(),
            distance * pitch.sin(),
        );

    let aspect = width as f32 / height as f32;
    let view = Mat4::look_at_rh(eye, center, Vec3::Z);
    let proj = Mat4::perspective_rh(
        45f32.to_radians(),
        aspect.max(0.0001),
        extent * 0.05,
        extent * 20.0,
    );
    let view_proj = proj * view;

    let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uniform buf"),
        contents: bytemuck::bytes_of(&Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
        }),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("uniform bg"),
        layout: &uniform_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
    });

    // Per-batch colour (single batch, light grey tint).
    let batch_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("batch buf"),
        contents: bytemuck::bytes_of(&BatchUniform {
            color: [0.75, 0.82, 0.90, 1.0],
        }),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let batch_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("batch bg"),
        layout: &batch_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &batch_buf,
                offset: 0,
                size: wgpu::BufferSize::new(std::mem::size_of::<BatchUniform>() as u64),
            }),
        }],
    });

    // Fallback 1x1 white texture.
    let fallback_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fallback tex"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &fallback_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[255u8, 255, 255, 255],
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let fallback_view = fallback_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let texture_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("texture bg"),
        layout: &texture_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&fallback_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });

    // ── 5. Offscreen render targets ──────────────────────────────────────
    let color_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("color target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: color_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: depth_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

    // ── 6. Render ────────────────────────────────────────────────────────
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("headless encoder"),
    });

    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("headless pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.08,
                        g: 0.09,
                        b: 0.11,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &uniform_bg, &[]);
        pass.set_bind_group(1, &batch_bg, &[]);
        pass.set_bind_group(2, &texture_bg, &[]);
        pass.set_vertex_buffer(0, vertex_buf.slice(..));
        pass.set_index_buffer(index_buf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.indices.len() as u32, 0, 0..1);
    }

    // ── 7. Copy to readback buffer ───────────────────────────────────────
    // Row alignment: wgpu requires `bytes_per_row` to be a multiple of 256.
    let bytes_per_pixel = 4u32;
    let unpadded_row = width * bytes_per_pixel;
    let padded_row = (unpadded_row + 255) & !255;

    let readback_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (padded_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &color_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &readback_buf,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    queue.submit(std::iter::once(encoder.finish()));

    // ── 8. Map and read back ─────────────────────────────────────────────
    let slice = readback_buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).ok();
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .context("map channel closed")?
        .context("buffer map failed")?;

    let data = slice.get_mapped_range();
    // Strip row padding.
    let mut pixels = Vec::with_capacity((width * height * bytes_per_pixel) as usize);
    for row in 0..height {
        let start = (row * padded_row) as usize;
        let end = start + unpadded_row as usize;
        pixels.extend_from_slice(&data[start..end]);
    }
    drop(data);
    readback_buf.unmap();

    // ── 9. Encode PNG ────────────────────────────────────────────────────
    use image::{codecs::png::PngEncoder, ImageEncoder};
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let writer = std::io::BufWriter::new(file);
    PngEncoder::new(writer)
        .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
        .context("PNG encode failed")?;

    println!("Rendered {width}x{height} to {}", path.display());
    Ok(())
}
