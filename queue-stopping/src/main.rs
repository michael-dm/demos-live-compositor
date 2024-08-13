use compositor_pipeline::{
    audio_mixer::{AudioChannels, AudioMixingParams, InputParams, MixingStrategy},
    pipeline::{
        self,
        input::{
            mp4::{Mp4Options, Source},
            InputOptions,
        },
        output::{RawAudioOptions, RawDataOutputOptions, RawVideoOptions},
        PipelineOutputEndCondition, RawDataReceiver, RegisterInputOptions, RegisterOutputOptions,
    },
    queue::{PipelineEvent, QueueInputOptions},
    Pipeline,
};
use compositor_render::{
    error::ErrorStack,
    scene::{Component, InputStreamComponent},
    Frame, FrameData, InputId, OutputId, Resolution,
};
use live_compositor::{config::read_config, state::ApiState};
use std::{path::PathBuf, sync::Arc, thread, time::Duration};
use winit::{
    dpi::PhysicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::{Fullscreen, WindowBuilder},
};

const BUNNY_FILE_PATH: &str = "BigBuckBunny.mp4";

const OUTPUT_RESOLUTION: Resolution = Resolution {
    width: 1280,
    height: 720,
};

fn root_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

struct RenderState {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

fn main() {
    tracing_subscriber::fmt().init();

    tracing::info!("Starting the coordinator application");

    let event_loop = EventLoop::new().unwrap();
    let window = WindowBuilder::new()
        .with_inner_size(PhysicalSize::new(
            OUTPUT_RESOLUTION.width as u32,
            OUTPUT_RESOLUTION.height as u32,
        ))
        .build(&event_loop)
        .unwrap();
    let window = Arc::new(window);

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });

    let surface = instance.create_surface(window.clone()).unwrap();

    let adapter = Arc::new(
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptionsBase {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .unwrap(),
    );

    let mut config = read_config();
    //config.queue_options.ahead_of_time_processing = true;
    config.adapter = Some(adapter.clone());
    let (state, _) = ApiState::new(config).unwrap_or_else(|err| {
        panic!(
            "Failed to start compositor: \n{}",
            ErrorStack::new(&err).into_string()
        );
    });
    let output_id = OutputId("output_1".into());
    let input_id = InputId("input_1".into());

    let output_options = RegisterOutputOptions {
        output_options: RawDataOutputOptions {
            video: Some(RawVideoOptions {
                resolution: OUTPUT_RESOLUTION,
            }),
            audio: None,
        },
        video: Some(pipeline::OutputVideoOptions {
            initial: Component::InputStream(InputStreamComponent {
                id: None,
                input_id: input_id.clone(),
            }),
            end_condition: pipeline::PipelineOutputEndCondition::Never,
        }),
        audio: None,
    };

    let input_options = RegisterInputOptions {
        input_options: InputOptions::Mp4(Mp4Options {
            source: Source::File(root_dir().join(BUNNY_FILE_PATH)),
        }),
        queue_options: QueueInputOptions {
            required: true,
            offset: Some(Duration::ZERO),
            buffer_duration: None,
        },
    };

    println!("Registering input");
    Pipeline::register_input(&state.pipeline, input_id.clone(), input_options).unwrap();

    println!("Getting wgpu context");
    let (wgpu_device, wgpu_queue) = state.pipeline.lock().unwrap().wgpu_ctx();

    println!("Registering output");
    let RawDataReceiver { video, audio } = state
        .pipeline
        .lock()
        .unwrap()
        .register_raw_data_output(output_id, output_options)
        .unwrap();

    println!("Starting pipeline");
    Pipeline::start(&state.pipeline);

    // Configure surface
    let size = window.inner_size();
    let surface_caps = surface.get_capabilities(&adapter);
    let surface_format = surface_caps
        .formats
        .iter()
        .copied()
        .find(|f| *f == wgpu::TextureFormat::Bgra8UnormSrgb)
        .expect("No suitable surface format found");

    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: size.width,
        height: size.height,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 0,
        alpha_mode: wgpu::CompositeAlphaMode::Auto,
        view_formats: vec![],
    };
    surface.configure(&wgpu_device, &config);

    let render_state = create_render_pipeline(&wgpu_device, surface_format);

    let video_receiver = video.unwrap();
    let mut close_requested = false;

    println!("Running event loop");
    event_loop
        .run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::Poll);
            match event {
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => {
                    close_requested = true;
                }
                Event::WindowEvent {
                    event: WindowEvent::Resized(new_size),
                    ..
                } => {
                    tracing::info!("Resizing surface to {:?}", new_size);
                    config.width = new_size.width;
                    config.height = new_size.height;
                    surface.configure(&wgpu_device, &config);
                }
                Event::WindowEvent {
                    event: WindowEvent::RedrawRequested,
                    ..
                } => {
                    if let Ok(PipelineEvent::Data(frame)) = video_receiver.try_recv() {
                        render_texture(&frame, &wgpu_device, &wgpu_queue, &surface, &render_state);
                        window.request_redraw();
                        tracing::info!("Received frame");
                    }
                }
                Event::AboutToWait => {
                    if close_requested {
                        elwt.exit();
                    }
                }
                _ => {}
            }
        })
        .unwrap();
}

fn create_render_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
) -> RenderState {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Vertex Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        label: Some("texture_bind_group_layout"),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Render Pipeline Layout"),
        bind_group_layouts: &[&bind_group_layout],
        push_constant_ranges: &[],
    });

    let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Render Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_main",
            buffers: &[], // No vertex buffers as we're using a full-screen triangle
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview: None,
    });

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());

    RenderState {
        render_pipeline,
        bind_group_layout,
        sampler,
    }
}

fn render_texture(
    frame: &Frame,
    device: &Arc<wgpu::Device>,
    queue: &Arc<wgpu::Queue>,
    surface: &wgpu::Surface,
    render_state: &RenderState,
) {
    let FrameData::Rgba8UnormWgpuTexture(texture) = &frame.data else {
        tracing::error!("Unexpected frame data format");
        return;
    };

    let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        layout: &render_state.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&render_state.sampler),
            },
        ],
        label: Some("conversion_bind_group"),
    });

    let frame = surface
        .get_current_texture()
        .expect("Failed to acquire next swap chain texture");
    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut command_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Command Encoder"),
    });

    {
        let mut render_pass = command_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            label: Some("Render Pass"),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        render_pass.set_pipeline(&render_state.render_pipeline);
        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.draw(0..3, 0..1); // Full-screen triangle
    }

    queue.submit(Some(command_encoder.finish()));
    frame.present();
}
