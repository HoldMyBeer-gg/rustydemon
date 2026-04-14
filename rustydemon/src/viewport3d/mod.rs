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

/// GPU-side resources that live for the whole app lifetime.
struct Viewport3dResources {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// Mesh pipeline + the latest uploaded mesh (lazy-rebuilt when the
/// callback sees a different `Arc` pointer).
struct MeshResources {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    /// Currently-uploaded mesh, keyed by Arc pointer for cache identity.
    cached: Option<UploadedMesh>,
}

struct UploadedMesh {
    key: usize,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    index_count: u32,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct MeshUniforms {
    view_proj: [[f32; 4]; 4],
}

const MESH_SHADER: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(pos, 1.0);
    out.world = pos;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Flat shading via screen-space derivatives — no per-vertex normals
    // needed. Light is a fixed direction; base color is parchment.
    let dx = dpdx(in.world);
    let dy = dpdy(in.world);
    let n = normalize(cross(dx, dy));
    let light_dir = normalize(vec3<f32>(0.4, 0.8, 0.5));
    let lambert = max(dot(n, light_dir), 0.0);
    let shaded = 0.25 + 0.75 * lambert;
    return vec4<f32>(vec3<f32>(shaded) * vec3<f32>(0.85, 0.82, 0.74), 1.0);
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

    // ── Mesh pipeline (real WMO geometry) ─────────────────────────────────────
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

    let mesh_pl_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("viewport3d mesh pl"),
        bind_group_layouts: &[&mesh_bgl],
        push_constant_ranges: &[],
    });

    let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("viewport3d mesh pipeline"),
        layout: Some(&mesh_pl_layout),
        vertex: wgpu::VertexState {
            module: &mesh_shader,
            entry_point: "vs_main",
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: (std::mem::size_of::<f32>() * 3) as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3],
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &mesh_shader,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Cw,
            cull_mode: Some(wgpu::Face::Back),
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    let mut renderer = render_state.renderer.write();
    renderer
        .callback_resources
        .insert(Arc::new(Viewport3dResources {
            pipeline,
            uniform_buffer,
            bind_group,
        }));
    renderer.callback_resources.insert(MeshResources {
        pipeline: mesh_pipeline,
        bind_group_layout: mesh_bgl,
        cached: None,
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
struct MeshCallback {
    mesh: Arc<Mesh3dCpu>,
    angle: f32,
    aspect: f32,
}

impl egui_wgpu::CallbackTrait for MeshCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get_mut::<MeshResources>() else {
            return Vec::new();
        };

        let key = Arc::as_ptr(&self.mesh) as usize;
        let needs_upload = match &res.cached {
            Some(c) => c.key != key,
            None => true,
        };

        if needs_upload {
            // Flatten positions to a tightly packed float buffer.
            let mut flat: Vec<f32> = Vec::with_capacity(self.mesh.positions.len() * 3);
            for p in &self.mesh.positions {
                flat.extend_from_slice(p);
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
                layout: &res.bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                }],
            });
            res.cached = Some(UploadedMesh {
                key,
                vertex_buf,
                index_buf,
                index_count: self.mesh.indices.len() as u32,
                uniform_buf,
                bind_group,
            });
        }

        // Compute view-projection from bbox + animated angle.
        let mn = Vec3::from(self.mesh.bbox_min);
        let mx = Vec3::from(self.mesh.bbox_max);
        let center = (mn + mx) * 0.5;
        let extent = (mx - mn).length().max(1.0);
        let radius = extent * 1.4;

        // WoW is Z-up — orbit in the XY plane, look down at Z = center.z.
        let eye = center
            + Vec3::new(
                radius * self.angle.cos(),
                radius * self.angle.sin(),
                radius * 0.45,
            );
        let view = Mat4::look_at_rh(eye, center, Vec3::Z);
        let proj = Mat4::perspective_rh(
            45f32.to_radians(),
            self.aspect.max(0.0001),
            extent * 0.05,
            extent * 10.0,
        );
        let view_proj = proj * view;

        if let Some(c) = &res.cached {
            let uniforms = MeshUniforms {
                view_proj: view_proj.to_cols_array_2d(),
            };
            queue.write_buffer(&c.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        }

        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(res) = resources.get::<MeshResources>() else {
            return;
        };
        let Some(c) = &res.cached else {
            return;
        };
        render_pass.set_pipeline(&res.pipeline);
        render_pass.set_bind_group(0, &c.bind_group, &[]);
        render_pass.set_vertex_buffer(0, c.vertex_buf.slice(..));
        render_pass.set_index_buffer(c.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        render_pass.draw_indexed(0..c.index_count, 0, 0..1);
    }
}

/// Render `mesh` into the inline preview pane using the existing wgpu
/// pipeline. Caller allocates the rect; we slowly auto-orbit in v0.
pub fn paint_mesh(ui: &mut egui::Ui, mesh: Arc<Mesh3dCpu>) {
    let available = ui.available_size();
    let size = Vec2::new(available.x.max(64.0), 320.0_f32.min(available.y.max(64.0)));
    let (rect, _response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let aspect = if rect.height() > 0.0 {
        rect.width() / rect.height()
    } else {
        1.0
    };
    let angle = (ui.ctx().input(|i| i.time) as f32) * 0.4;

    ui.painter().add(egui_wgpu::Callback::new_paint_callback(
        rect,
        MeshCallback {
            mesh,
            angle,
            aspect,
        },
    ));

    ui.ctx().request_repaint();
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
