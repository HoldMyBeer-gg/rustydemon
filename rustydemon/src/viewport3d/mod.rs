//! 3D viewport spike.
//!
//! Stage 1 of the WMO render path: prove that wgpu rendering inside an
//! egui paint callback works end-to-end before any model data is wired
//! in. Renders a single rotating triangle in a windowed viewport that
//! the user can toggle from `View → 3D Test Viewport`.
//!
//! Resource ownership follows the egui_wgpu typemap pattern: the pipeline
//! and uniform buffer live in `RenderState::renderer.callback_resources`,
//! not on `CascExplorerApp`. The per-frame rotation is the only thing the
//! callback struct itself carries.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu;
use egui::Vec2;
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::preview::Mesh3dCpu;

/// Stride of one per-batch uniform within the dynamic-offset buffer.
/// Must be ≥ size_of::<BatchUniform>() AND a multiple of
/// `min_uniform_buffer_offset_alignment` (≤ 256 on every backend we
/// care about). Using a fixed 256 keeps the math and pipeline layout
/// constant across devices.
const BATCH_UNIFORM_STRIDE: u64 = 256;

/// GPU-side resources that live for the whole app lifetime.
struct Viewport3dResources {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// Mesh pipeline + the latest uploaded mesh (lazy-rebuilt when the
/// callback sees a different `Arc` pointer).
///
/// v1 architecture: the scene renders into an offscreen colour+depth
/// target sized to the viewport rect, then a fullscreen-triangle blit
/// pipeline samples that colour texture into egui's render pass. This
/// is what gives us proper depth occlusion despite egui's main render
/// pass having no depth attachment.
struct MeshResources {
    mesh_pipeline: wgpu::RenderPipeline,
    mesh_bgl: wgpu::BindGroupLayout,
    batch_bgl: wgpu::BindGroupLayout,
    /// Bind group layout for the per-material texture (slot 2). Holds
    /// one sampled texture + one filtering sampler.
    texture_bgl: wgpu::BindGroupLayout,
    /// Linear sampler shared by every material texture binding.
    texture_sampler: wgpu::Sampler,
    /// 1x1 white fallback texture + bind group used by batches whose
    /// material has no texture data (decode failed, missing FDID, or
    /// the mesh has no material info at all). The texture handle is
    /// kept alongside the bind group so the GPU resource isn't dropped
    /// out from under it.
    #[allow(dead_code)]
    fallback_texture: wgpu::Texture,
    fallback_bind_group: wgpu::BindGroup,
    blit_pipeline: wgpu::RenderPipeline,
    blit_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    offscreen_format: wgpu::TextureFormat,
    /// Currently-uploaded mesh, keyed by Arc pointer for cache identity.
    cached: Option<UploadedMesh>,
    /// Cached offscreen target — recreated when the rect size changes.
    offscreen: Option<Offscreen>,
}

struct UploadedMesh {
    key: usize,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// Per-batch colour uniforms packed at `BATCH_UNIFORM_STRIDE` apart.
    /// Held to keep the GPU-side allocation alive for as long as the
    /// bind group references it; never read directly after construction.
    #[allow(dead_code)]
    batch_uniform_buf: wgpu::Buffer,
    batch_bind_group: wgpu::BindGroup,
    /// One bind group per material (slot 2 — sampled texture). Indexed
    /// by `MeshBatch::material_id`. Populated at upload time. Empty
    /// when the source had no material info; in that case every batch
    /// uses the fallback bind group.
    material_bind_groups: Vec<wgpu::BindGroup>,
    /// Held to keep the underlying material textures alive while the
    /// bind groups reference them. Never read directly.
    #[allow(dead_code)]
    material_textures: Vec<wgpu::Texture>,
    batches: Vec<UploadedBatch>,
    /// Per-mesh camera state — resets when a new selection uploads.
    camera: CameraState,
}

/// Orbit camera state. Yaw rotates around the Z axis (WoW up), pitch
/// elevates the eye above the bbox center, distance scales the orbit
/// radius around the bbox extent. Defaults are tuned to give a sane
/// initial framing of any model.
///
/// `pan` offsets the orbit center in view-space (right, up) so the
/// user can shift the framing without changing the orbit radius.
/// Shift+drag triggers panning; plain drag triggers orbit.
#[derive(Clone, Copy)]
struct CameraState {
    yaw: f32,
    pitch: f32,
    distance_mul: f32,
    /// View-space pan offset (right, up) in world units.
    pan: [f32; 2],
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            yaw: 0.6,
            pitch: 0.45,
            distance_mul: 1.4,
            pan: [0.0; 2],
        }
    }
}

#[derive(Clone, Copy)]
struct UploadedBatch {
    start_index: u32,
    index_count: u32,
}

struct Offscreen {
    width: u32,
    height: u32,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    blit_bind_group: wgpu::BindGroup,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct MeshUniforms {
    view_proj: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct BatchUniform {
    color: [f32; 4],
}

/// Map a material id to a stable, pleasant colour. Hue jittered by the
/// id so adjacent ids visually separate; saturation/lightness fixed for
/// a coherent palette.
fn material_color(material_id: u32) -> [f32; 4] {
    // Cheap integer hash → hue in [0, 1).
    let mut x = material_id.wrapping_mul(2654435761);
    x ^= x >> 16;
    let hue = (x as f32 / u32::MAX as f32).fract();
    hsv_to_rgb(hue, 0.45, 0.85)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 4] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match (i as i32).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [r, g, b, 1.0]
}

const BLIT_SHADER: &str = r#"
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_smp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
    var ps = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 2.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(2.0, 0.0),
    );
    var out: VOut;
    out.pos = vec4<f32>(ps[i], 0.0, 1.0);
    out.uv = uvs[i];
    return out;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    return textureSample(src_tex, src_smp, in.uv);
}
"#;

const MESH_SHADER: &str = r#"
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
    // Flat shading via screen-space derivatives — no per-vertex normals
    // needed. Sampled texture provides base colour; b.color acts as a
    // tint (white when materials are present, hash-coloured otherwise).
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

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    angle: f32,
    aspect: f32,
    _pad: [f32; 2],
}

const SHADER_SRC: &str = r#"
struct Uniforms {
    angle: f32,
    aspect: f32,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>( 0.0,  0.6),
        vec2<f32>(-0.6, -0.5),
        vec2<f32>( 0.6, -0.5),
    );
    var colors = array<vec3<f32>, 3>(
        vec3<f32>(1.0, 0.2, 0.3),
        vec3<f32>(0.2, 1.0, 0.4),
        vec3<f32>(0.3, 0.4, 1.0),
    );
    let p = positions[vid];
    let c = cos(u.angle);
    let s = sin(u.angle);
    let r = vec2<f32>(p.x * c - p.y * s, p.x * s + p.y * c);
    // Aspect-correct so the triangle isn't stretched in non-square windows.
    let corrected = vec2<f32>(r.x / max(u.aspect, 0.0001), r.y);
    var out: VsOut;
    out.pos = vec4<f32>(corrected, 0.0, 1.0);
    out.color = colors[vid];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
"#;

/// Install GPU resources into the egui_wgpu renderer's typemap.
/// Call once from `CascExplorerApp::new` if a wgpu RenderState is available.
pub fn init(render_state: &egui_wgpu::RenderState) {
    let device = &render_state.device;
    let target_format = render_state.target_format;

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("viewport3d shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
    });

    let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("viewport3d uniforms"),
        contents: bytemuck::bytes_of(&Uniforms {
            angle: 0.0,
            aspect: 1.0,
            _pad: [0.0; 2],
        }),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewport3d bgl"),
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

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("viewport3d bg"),
        layout: &bind_group_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("viewport3d pl"),
        bind_group_layouts: &[&bind_group_layout],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("viewport3d pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_main",
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    // ── Mesh pipeline (real WMO geometry, renders to offscreen) ───────────────
    // Use the same colour format for offscreen as for the egui surface so
    // the blit pipeline doesn't need any format conversion.
    let offscreen_format = target_format;
    let depth_format = wgpu::TextureFormat::Depth32Float;

    let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("viewport3d mesh shader"),
        source: wgpu::ShaderSource::Wgsl(MESH_SHADER.into()),
    });

    let mesh_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewport3d mesh bgl"),
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

    // Per-batch colour, addressed via dynamic offset into a single buffer.
    let batch_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewport3d batch bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<BatchUniform>() as u64),
            },
            count: None,
        }],
    });

    // Per-material texture + sampler.
    let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewport3d texture bgl"),
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

    let mesh_pl_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("viewport3d mesh pl"),
        bind_group_layouts: &[&mesh_bgl, &batch_bgl, &texture_bgl],
        push_constant_ranges: &[],
    });

    let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("viewport3d mesh pipeline"),
        layout: Some(&mesh_pl_layout),
        vertex: wgpu::VertexState {
            module: &mesh_shader,
            entry_point: "vs_main",
            buffers: &[wgpu::VertexBufferLayout {
                // 5 floats per vertex: position(xyz) + uv.
                array_stride: (std::mem::size_of::<f32>() * 5) as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &mesh_shader,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format: offscreen_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Cw,
            // No back-face culling — M2/Granny models have mixed
            // winding (hair, capes, two-sided geometry), and WMO
            // groups occasionally do too.  The flat-shading normal
            // derivation via dpdx/dpdy handles both sides correctly.
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
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

    // ── Blit pipeline (offscreen colour → egui render pass) ───────────────────
    let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("viewport3d blit shader"),
        source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()),
    });

    let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewport3d blit bgl"),
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

    let blit_pl_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("viewport3d blit pl"),
        bind_group_layouts: &[&blit_bgl],
        push_constant_ranges: &[],
    });

    let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("viewport3d blit pipeline"),
        layout: Some(&blit_pl_layout),
        vertex: wgpu::VertexState {
            module: &blit_shader,
            entry_point: "vs",
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &blit_shader,
            entry_point: "fs",
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("viewport3d blit sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    let mut renderer = render_state.renderer.write();
    renderer
        .callback_resources
        .insert(Arc::new(Viewport3dResources {
            pipeline,
            uniform_buffer,
            bind_group,
        }));
    // Per-material texture sampler (linear, repeat — WoW UVs frequently
    // exceed [0,1] for tiled textures, e.g. floor planks).
    let texture_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("viewport3d material sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    // 1×1 white fallback texture for batches whose material has no
    // valid BLP. Created here so it lives for the whole app lifetime.
    let fallback_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("viewport3d fallback white"),
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
    render_state.queue.write_texture(
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
    let fallback_view = fallback_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let fallback_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("viewport3d fallback bg"),
        layout: &texture_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&fallback_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&texture_sampler),
            },
        ],
    });
    renderer.callback_resources.insert(MeshResources {
        mesh_pipeline,
        mesh_bgl,
        batch_bgl,
        texture_bgl,
        texture_sampler,
        fallback_texture: fallback_tex,
        fallback_bind_group,
        blit_pipeline,
        blit_bgl,
        sampler,
        offscreen_format,
        cached: None,
        offscreen: None,
    });
}

/// Per-frame callback handed to egui_wgpu. Carries the rotation angle and
/// viewport aspect ratio; everything else lives in the typemap.
struct TriangleCallback {
    angle: f32,
    aspect: f32,
}

impl egui_wgpu::CallbackTrait for TriangleCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(res) = resources.get::<Arc<Viewport3dResources>>() {
            let uniforms = Uniforms {
                angle: self.angle,
                aspect: self.aspect,
                _pad: [0.0; 2],
            };
            queue.write_buffer(&res.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(res) = resources.get::<Arc<Viewport3dResources>>() {
            render_pass.set_pipeline(&res.pipeline);
            render_pass.set_bind_group(0, &res.bind_group, &[]);
            render_pass.draw(0..3, 0..1);
        }
    }
}

/// Per-frame callback that draws an indexed WMO mesh.
///
/// Carries the scene state (`mesh`, camera deltas) plus the *pixel*
/// size of the egui rect we're painting into so `prepare()` can size
/// the offscreen render target correctly. The camera *deltas* (not the
/// absolute state) are passed through here because the absolute state
/// lives on `UploadedMesh` and survives between frames; the callback
/// just applies this frame's input.
struct MeshCallback {
    mesh: Arc<Mesh3dCpu>,
    yaw_delta: f32,
    pitch_delta: f32,
    zoom_delta: f32,
    pan_x_delta: f32,
    pan_y_delta: f32,
    pixel_width: u32,
    pixel_height: u32,
}

impl egui_wgpu::CallbackTrait for MeshCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get_mut::<MeshResources>() else {
            return Vec::new();
        };

        // ── 1. Upload mesh on first sight or when the selection changes ───────
        let key = Arc::as_ptr(&self.mesh) as usize;
        let needs_upload = match &res.cached {
            Some(c) => c.key != key,
            None => true,
        };
        if needs_upload {
            // Interleave position + uv into a single packed vertex buffer.
            // 5 floats per vertex; pad missing UVs with (0, 0).
            let vert_count = self.mesh.positions.len();
            let mut flat: Vec<f32> = Vec::with_capacity(vert_count * 5);
            for i in 0..vert_count {
                let p = self.mesh.positions[i];
                let uv = self.mesh.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
                flat.extend_from_slice(&[p[0], p[1], p[2], uv[0], uv[1]]);
            }
            let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("wmo vertex buf"),
                contents: bytemuck::cast_slice(&flat),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("wmo index buf"),
                contents: bytemuck::cast_slice(&self.mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("wmo uniform buf"),
                size: std::mem::size_of::<MeshUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("wmo bg"),
                layout: &res.mesh_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                }],
            });

            // Build the dynamic-offset batch uniform buffer: one
            // BATCH_UNIFORM_STRIDE-sized slot per batch. When materials
            // are present the slot holds white (texture provides colour);
            // otherwise it holds a hash colour derived from material_id
            // so single groups without a root still show structural
            // material boundaries.
            let has_materials = !self.mesh.materials.is_empty();
            let batch_count = self.mesh.batches.len().max(1);
            let batch_buf_size = BATCH_UNIFORM_STRIDE * batch_count as u64;
            let mut batch_bytes = vec![0u8; batch_buf_size as usize];
            for (i, b) in self.mesh.batches.iter().enumerate() {
                let color = if has_materials {
                    [1.0, 1.0, 1.0, 1.0]
                } else {
                    material_color(b.material_id)
                };
                let bu = BatchUniform { color };
                let off = i * BATCH_UNIFORM_STRIDE as usize;
                batch_bytes[off..off + std::mem::size_of::<BatchUniform>()]
                    .copy_from_slice(bytemuck::bytes_of(&bu));
            }
            let batch_uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("wmo batch uniforms"),
                contents: &batch_bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let batch_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("wmo batch bg"),
                layout: &res.batch_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &batch_uniform_buf,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<BatchUniform>() as u64),
                    }),
                }],
            });

            let batches: Vec<UploadedBatch> = self
                .mesh
                .batches
                .iter()
                .map(|b| UploadedBatch {
                    start_index: b.start_index,
                    index_count: b.index_count,
                })
                .collect();

            // ── Upload per-material textures + build bind groups ──────────
            // Each MeshMaterial becomes one wgpu::Texture and one bind
            // group bound to slot 2 of the mesh pipeline. Materials whose
            // BLP failed to decode get the global fallback bind group
            // (white) so the per-batch tint colour shows through.
            let mut material_textures: Vec<wgpu::Texture> = Vec::new();
            let mut material_bind_groups: Vec<wgpu::BindGroup> = Vec::new();
            for mat in &self.mesh.materials {
                if let Some(rgba) = mat.rgba.as_deref() {
                    let tex = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("wmo material tex"),
                        size: wgpu::Extent3d {
                            width: mat.width,
                            height: mat.height,
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
                            texture: &tex,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        rgba,
                        wgpu::ImageDataLayout {
                            offset: 0,
                            bytes_per_row: Some(mat.width * 4),
                            rows_per_image: Some(mat.height),
                        },
                        wgpu::Extent3d {
                            width: mat.width,
                            height: mat.height,
                            depth_or_array_layers: 1,
                        },
                    );
                    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
                    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("wmo material bg"),
                        layout: &res.texture_bgl,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&res.texture_sampler),
                            },
                        ],
                    });
                    material_textures.push(tex);
                    material_bind_groups.push(bg);
                } else {
                    // Decode failed — emit a placeholder slot. The render
                    // loop sees the missing entry and uses the fallback.
                    // We still push a dummy texture/bind group to keep
                    // indexing aligned with `mesh.materials`.
                    material_textures.push(device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("wmo material placeholder"),
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
                    }));
                    // Placeholder bind group: clone of fallback semantics.
                    // We'll detect at draw time and substitute.
                    let view = material_textures
                        .last()
                        .unwrap()
                        .create_view(&wgpu::TextureViewDescriptor::default());
                    material_bind_groups.push(device.create_bind_group(
                        &wgpu::BindGroupDescriptor {
                            label: Some("wmo material placeholder bg"),
                            layout: &res.texture_bgl,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(&view),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::Sampler(&res.texture_sampler),
                                },
                            ],
                        },
                    ));
                }
            }

            res.cached = Some(UploadedMesh {
                key,
                vertex_buf,
                index_buf,
                uniform_buf,
                bind_group,
                batch_uniform_buf,
                batch_bind_group,
                material_bind_groups,
                material_textures,
                batches,
                camera: CameraState::default(),
            });
        }

        // Apply this frame's input deltas to the persistent camera state.
        if let Some(c) = res.cached.as_mut() {
            c.camera.yaw += self.yaw_delta;
            c.camera.pitch = (c.camera.pitch + self.pitch_delta).clamp(
                -std::f32::consts::FRAC_PI_2 + 0.05,
                std::f32::consts::FRAC_PI_2 - 0.05,
            );
            c.camera.distance_mul =
                (c.camera.distance_mul * (1.0 - self.zoom_delta)).clamp(0.2, 10.0);
            c.camera.pan[0] += self.pan_x_delta;
            c.camera.pan[1] += self.pan_y_delta;
        }

        // ── 2. (Re)create offscreen targets if the rect size changed ──────────
        let width = self.pixel_width.max(1);
        let height = self.pixel_height.max(1);
        let needs_resize = match &res.offscreen {
            Some(o) => o.width != width || o.height != height,
            None => true,
        };
        if needs_resize {
            let color = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("wmo offscreen color"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: res.offscreen_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let depth = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("wmo offscreen depth"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
            let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
            let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("wmo blit bg"),
                layout: &res.blit_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&color_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&res.sampler),
                    },
                ],
            });
            res.offscreen = Some(Offscreen {
                width,
                height,
                color_view,
                depth_view,
                blit_bind_group,
            });
        }

        // ── 3. Update view-projection uniform ─────────────────────────────────
        let aspect = width as f32 / height as f32;
        let mn = Vec3::from(self.mesh.bbox_min);
        let mx = Vec3::from(self.mesh.bbox_max);
        let bbox_center = (mn + mx) * 0.5;
        let extent = (mx - mn).length().max(1.0);

        let cam = res.cached.as_ref().map(|c| c.camera).unwrap_or_default();
        let radius = extent * cam.distance_mul;
        let cos_p = cam.pitch.cos();
        let forward = Vec3::new(
            cos_p * cam.yaw.cos(),
            cos_p * cam.yaw.sin(),
            cam.pitch.sin(),
        );
        let world_up = Vec3::Z;
        let right = forward.cross(world_up).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();

        // Apply pan offset in view space (right + up).
        let center = bbox_center + right * cam.pan[0] + up * cam.pan[1];
        let eye = center + forward * radius;

        let view = Mat4::look_at_rh(eye, center, Vec3::Z);
        let proj = Mat4::perspective_rh(
            45f32.to_radians(),
            aspect.max(0.0001),
            extent * 0.05,
            extent * 20.0,
        );
        let view_proj = proj * view;
        if let Some(c) = &res.cached {
            let uniforms = MeshUniforms {
                view_proj: view_proj.to_cols_array_2d(),
            };
            queue.write_buffer(&c.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        }

        // ── 4. Render scene into the offscreen target ─────────────────────────
        if let (Some(c), Some(o)) = (&res.cached, &res.offscreen) {
            let mut pass = egui_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("wmo offscreen pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &o.color_view,
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
                    view: &o.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&res.mesh_pipeline);
            pass.set_bind_group(0, &c.bind_group, &[]);
            pass.set_vertex_buffer(0, c.vertex_buf.slice(..));
            pass.set_index_buffer(c.index_buf.slice(..), wgpu::IndexFormat::Uint32);
            for (i, b) in c.batches.iter().enumerate() {
                let dyn_offset = (i as u64 * BATCH_UNIFORM_STRIDE) as u32;
                pass.set_bind_group(1, &c.batch_bind_group, &[dyn_offset]);

                // Pick the material's texture bind group, or fall back
                // to white when the batch has no valid material slot.
                let mat_bg = self
                    .mesh
                    .batches
                    .get(i)
                    .and_then(|mb| {
                        let mid = mb.material_id as usize;
                        // Only honour the material if the source mesh
                        // had material info AND the index is in range
                        // AND the original CPU-side material had RGBA.
                        if !self.mesh.materials.is_empty()
                            && mid < self.mesh.materials.len()
                            && self.mesh.materials[mid].rgba.is_some()
                        {
                            c.material_bind_groups.get(mid)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(&res.fallback_bind_group);
                pass.set_bind_group(2, mat_bg, &[]);

                let start = b.start_index;
                let end = start + b.index_count;
                pass.draw_indexed(start..end, 0, 0..1);
            }
        }

        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(res) = resources.get::<MeshResources>() else {
            return;
        };
        let Some(o) = &res.offscreen else {
            return;
        };

        // Constrain the fullscreen-triangle blit to the painter rect via
        // viewport, and clip to the visible portion via scissor — the
        // scissor matters when our rect is partially scrolled out of
        // view inside a parent ScrollArea, otherwise the offscreen blit
        // would bleed over neighbouring widgets.
        let vp = info.viewport_in_pixels();
        render_pass.set_viewport(
            vp.left_px as f32,
            vp.top_px as f32,
            vp.width_px as f32,
            vp.height_px as f32,
            0.0,
            1.0,
        );
        let cr = info.clip_rect_in_pixels();
        render_pass.set_scissor_rect(
            cr.left_px.max(0) as u32,
            cr.top_px.max(0) as u32,
            cr.width_px.max(0) as u32,
            cr.height_px.max(0) as u32,
        );
        render_pass.set_pipeline(&res.blit_pipeline);
        render_pass.set_bind_group(0, &o.blit_bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

/// Render `mesh` into the inline preview pane. Allocates a fixed-height
/// rect, reads drag + scroll input for the orbit camera, computes the
/// equivalent pixel size from the egui DPI scale, and hands everything
/// off to [`MeshCallback`].
///
/// - **Drag** = orbit (yaw/pitch)
/// - **Shift+Drag** = pan (translate the orbit center)
/// - **Scroll** = zoom
pub fn paint_mesh(ui: &mut egui::Ui, mesh: Arc<Mesh3dCpu>) {
    let width = ui.available_width().max(64.0);
    let size = Vec2::new(width, 240.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());

    let drag = response.drag_delta();
    let shift_held = ui.input(|i| i.modifiers.shift);

    // Shift+drag = pan, plain drag = orbit.
    let (yaw_delta, pitch_delta, pan_x_delta, pan_y_delta) = if shift_held {
        // Pan speed scaled to the viewport so it feels consistent
        // across model sizes.  The 0.005 factor gives ~1 world-unit
        // per full-width drag at distance_mul=1.
        (0.0, 0.0, drag.x * 0.005, -drag.y * 0.005)
    } else {
        (-drag.x * 0.01, -drag.y * 0.01, 0.0, 0.0)
    };

    // Scroll wheel → zoom multiplier. Only consume scroll while hovered
    // so it doesn't fight the surrounding scroll area.
    let zoom_delta = if response.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        scroll * 0.0015
    } else {
        0.0
    };

    let ppp = ui.ctx().pixels_per_point();
    let pixel_width = (rect.width() * ppp).round().max(1.0) as u32;
    let pixel_height = (rect.height() * ppp).round().max(1.0) as u32;

    ui.painter().add(egui_wgpu::Callback::new_paint_callback(
        rect,
        MeshCallback {
            mesh,
            yaw_delta,
            pitch_delta,
            zoom_delta,
            pan_x_delta,
            pan_y_delta,
            pixel_width,
            pixel_height,
        },
    ));

    // Keep repainting only while interacting; static frames don't need
    // continuous repaint now that the auto-rotate is gone.
    if response.dragged() || response.hovered() {
        ui.ctx().request_repaint();
    }
}

/// Show the spike viewport as a movable, resizable egui window.
/// `open` is mutated to false when the user closes the window.
pub fn show_window(ctx: &egui::Context, open: &mut bool) {
    if !*open {
        return;
    }

    egui::Window::new("3D Test Viewport")
        .open(open)
        .default_size([400.0, 400.0])
        .resizable(true)
        .show(ctx, |ui| {
            // Rotate at ~36°/sec. Using ctx time keeps it independent of
            // frame rate and survives the per-frame callback rebuild.
            let angle = (ctx.input(|i| i.time) as f32) * std::f32::consts::FRAC_PI_2;

            let available = ui.available_size();
            let size = Vec2::new(available.x.max(64.0), available.y.max(64.0));
            let (rect, _response) = ui.allocate_exact_size(size, egui::Sense::hover());
            let aspect = if rect.height() > 0.0 {
                rect.width() / rect.height()
            } else {
                1.0
            };

            ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                rect,
                TriangleCallback { angle, aspect },
            ));

            // Repaint continuously so the rotation animates.
            ctx.request_repaint();
        });
}
