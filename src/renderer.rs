use crate::mesh::{ChaperoneMesh, ChaperoneVertex};
use anyhow::{bail, Result};
use ash::vk;
use bytemuck::{Pod, Zeroable};
use glam::Mat4;

// This struct is pushed into the vert for rendering
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct PushConstants {
    view_proj: [[f32; 4]; 4],
    colour: [f32; 4],
    opacity: f32,
    line_width: f32,
    grid_spacing: f32,
}

unsafe fn create_shader_module(device: &ash::Device, spirv: &[u8]) -> Result<vk::ShaderModule> {
    if !spirv.len().is_multiple_of(4) {
        bail!("SPIR-V bytecode length is not a multiple of 4");
    }

    // Chunk this from a &[u8;4] into a u32
    let code: Vec<u32> = spirv
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();

    let info = vk::ShaderModuleCreateInfo::default().code(&code);
    Ok(unsafe { device.create_shader_module(&info, None) }?)
}

fn find_memory_type(
    props: &vk::PhysicalDeviceMemoryProperties,
    type_filter: u32,
    required: vk::MemoryPropertyFlags,
) -> Result<u32> {
    for i in 0..props.memory_type_count {
        if (type_filter & (1 << i)) != 0
            && props.memory_types[i as usize]
                .property_flags
                .contains(required)
        {
            return Ok(i);
        }
    }
    bail!("Failed to find suitable memory type");
}

pub struct EyeSwapChain {
    pub image_views: Vec<vk::ImageView>,
    pub framebuffers: Vec<vk::Framebuffer>,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
}

pub struct ChaperoneRenderer {
    pub render_pass: vk::RenderPass,
    pub depth_view: vk::ImageView,

    device: ash::Device, // shared reference clone (cheap with Arc in real code)
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    vertex_buf: vk::Buffer,
    vertex_mem: vk::DeviceMemory,
    index_buf: vk::Buffer,
    index_mem: vk::DeviceMemory,
    index_count: u32,
    depth_image: vk::Image,
    depth_mem: vk::DeviceMemory,
    cmd_pool: vk::CommandPool,
    cmd_bufs: Vec<vk::CommandBuffer>,
}

impl ChaperoneRenderer {
    pub unsafe fn new(
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        device: ash::Device,
        queue_family: u32,
        mesh: &ChaperoneMesh,
        swap_chain: &EyeSwapChain,
    ) -> Result<Self> {
        let mem_props = unsafe { instance.get_physical_device_memory_properties(physical_device) };

        let color_attach = vk::AttachmentDescription::default()
            .format(swap_chain.format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let depth_attach = vk::AttachmentDescription::default()
            .format(vk::Format::D32_SFLOAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);

        let color_ref = vk::AttachmentReference {
            attachment: 0,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        };
        let depth_ref = vk::AttachmentReference {
            attachment: 1,
            layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
        };

        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(std::slice::from_ref(&color_ref))
            .depth_stencil_attachment(&depth_ref);

        let attachments = [color_attach, depth_attach];
        let render_pass = unsafe {
            device.create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(&attachments)
                    .subpasses(std::slice::from_ref(&subpass)),
                None,
            )
        }?;

        // Depth image (shared across all framebuffers for this eye)
        let (depth_image, depth_mem) = {
            let info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::D32_SFLOAT)
                .extent(vk::Extent3D {
                    width: swap_chain.extent.width,
                    height: swap_chain.extent.height,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
                .initial_layout(vk::ImageLayout::UNDEFINED);

            let img = unsafe { device.create_image(&info, None) }?;
            let req = unsafe { device.get_image_memory_requirements(img) };
            let mem_idx = find_memory_type(
                &mem_props,
                req.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )?;

            let mem = unsafe {
                device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(req.size)
                        .memory_type_index(mem_idx),
                    None,
                )
            }?;

            unsafe { device.bind_image_memory(img, mem, 0) }?;
            (img, mem)
        };

        let depth_view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(depth_image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::D32_SFLOAT)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::DEPTH,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    }),
                None,
            )
        }?;

        // Load the Shaders (built inside build.rs)
        let vert_spv = include_bytes!(concat!(env!("OUT_DIR"), "/shaders/chaperone.vert.spv"));
        let frag_spv = include_bytes!(concat!(env!("OUT_DIR"), "/shaders/chaperone.frag.spv"));
        let vert_mod = unsafe { create_shader_module(&device, vert_spv) }?;
        let frag_mod = unsafe { create_shader_module(&device, frag_spv) }?;

        // Define the Entry Point
        let entry = c"main";

        // Set up the pipeline layout
        let pc_range = vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
            offset: 0,
            size: size_of::<PushConstants>() as u32,
        };
        let pipeline_layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .push_constant_ranges(std::slice::from_ref(&pc_range)),
                None,
            )
        }?;

        // Vertex input..
        let binding = vk::VertexInputBindingDescription {
            binding: 0,
            stride: size_of::<ChaperoneVertex>() as u32,
            input_rate: vk::VertexInputRate::VERTEX,
        };
        let attrs = [
            vk::VertexInputAttributeDescription {
                location: 0,
                binding: 0,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                location: 1,
                binding: 0,
                format: vk::Format::R32_SFLOAT,
                offset: 12,
            },
            vk::VertexInputAttributeDescription {
                location: 2,
                binding: 0,
                format: vk::Format::R32_SFLOAT,
                offset: 16,
            },
        ];

        // Prepare the Graphics Pipeline for our Shaders
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert_mod)
                .name(entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag_mod)
                .name(entry),
        ];

        // Configure for Alpha blending
        let blend_attach = vk::PipelineColorBlendAttachmentState {
            blend_enable: vk::TRUE,
            src_color_blend_factor: vk::BlendFactor::SRC_ALPHA,
            dst_color_blend_factor: vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
            color_blend_op: vk::BlendOp::ADD,
            src_alpha_blend_factor: vk::BlendFactor::ONE,
            dst_alpha_blend_factor: vk::BlendFactor::ZERO,
            alpha_blend_op: vk::BlendOp::ADD,
            color_write_mask: vk::ColorComponentFlags::RGBA,
        };

        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: swap_chain.extent.width as f32,
            height: swap_chain.extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };

        let scissor = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: swap_chain.extent,
        };

        // Finally, build the full graphics pipeline
        let pipeline = unsafe {
            device
                .create_graphics_pipelines(
                    vk::PipelineCache::null(),
                    &[vk::GraphicsPipelineCreateInfo::default()
                        .stages(&stages)
                        .vertex_input_state(
                            &vk::PipelineVertexInputStateCreateInfo::default()
                                .vertex_binding_descriptions(std::slice::from_ref(&binding))
                                .vertex_attribute_descriptions(&attrs),
                        )
                        .input_assembly_state(
                            &vk::PipelineInputAssemblyStateCreateInfo::default()
                                .topology(vk::PrimitiveTopology::TRIANGLE_LIST),
                        )
                        .viewport_state(
                            &vk::PipelineViewportStateCreateInfo::default()
                                .viewports(std::slice::from_ref(&viewport))
                                .scissors(std::slice::from_ref(&scissor)),
                        )
                        .rasterization_state(
                            &vk::PipelineRasterizationStateCreateInfo::default()
                                .polygon_mode(vk::PolygonMode::FILL)
                                .cull_mode(vk::CullModeFlags::NONE) // visible from both sides
                                .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
                                .line_width(1.0),
                        )
                        .multisample_state(
                            &vk::PipelineMultisampleStateCreateInfo::default()
                                .rasterization_samples(vk::SampleCountFlags::TYPE_1),
                        )
                        .depth_stencil_state(
                            &vk::PipelineDepthStencilStateCreateInfo::default()
                                .depth_test_enable(true)
                                .depth_write_enable(true)
                                .depth_compare_op(vk::CompareOp::LESS),
                        )
                        .color_blend_state(
                            &vk::PipelineColorBlendStateCreateInfo::default()
                                .attachments(std::slice::from_ref(&blend_attach)),
                        )
                        .layout(pipeline_layout)
                        .render_pass(render_pass)
                        .subpass(0)],
                    None,
                )
                .map_err(|(_, e)| e)
        }?[0];

        // We shouldn't need the shaders anymore, communication with them will happen in the
        // command buffer
        unsafe {
            device.destroy_shader_module(vert_mod, None);
            device.destroy_shader_module(frag_mod, None);
        }

        // Send up the Vertex and index data
        let vertex_data = bytemuck::cast_slice::<ChaperoneVertex, u8>(&mesh.vertices);
        let (vertex_buf, vertex_mem) = unsafe {
            upload_buffer(
                &device,
                &mem_props,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                vertex_data,
            )
        }?;

        let index_data = bytemuck::cast_slice::<u32, u8>(&mesh.indices);
        let (index_buf, index_mem) = unsafe {
            upload_buffer(
                &device,
                &mem_props,
                vk::BufferUsageFlags::INDEX_BUFFER,
                index_data,
            )
        }?;

        // Configure the command pool, and command buffer
        let cmd_pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
        }?;
        let cmd_bufs = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(cmd_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(swap_chain.image_views.len() as u32),
            )
        }?;

        // And we're done.
        Ok(Self {
            device,
            render_pass,
            pipeline_layout,
            pipeline,
            vertex_buf,
            vertex_mem,
            index_buf,
            index_mem,
            index_count: mesh.indices.len() as u32,
            depth_image,
            depth_mem,
            depth_view,
            cmd_pool,
            cmd_bufs,
        })
    }

    /// Record and return a command buffer for `image_index`.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn record(
        &self,
        image_index: usize,
        framebuffer: vk::Framebuffer,
        extent: vk::Extent2D,
        view_proj: Mat4,

        // TODO: I should probably group these params into a struct..
        opacity: f32,
        colour: [f32; 4],
        line_width: f32,
        grid_spacing: f32,
    ) -> Result<vk::CommandBuffer> {
        let cb = self.cmd_bufs[image_index];
        let dev = &self.device;
        unsafe { dev.reset_command_buffer(cb, vk::CommandBufferResetFlags::empty()) }?;
        unsafe {
            dev.begin_command_buffer(
                cb,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
        }?;

        let clear_values = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 0.0],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];

        unsafe {
            dev.cmd_begin_render_pass(
                cb,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(framebuffer)
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D::default(),
                        extent,
                    })
                    .clear_values(&clear_values),
                vk::SubpassContents::INLINE,
            );

            dev.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            dev.cmd_bind_vertex_buffers(cb, 0, &[self.vertex_buf], &[0]);
            dev.cmd_bind_index_buffer(cb, self.index_buf, 0, vk::IndexType::UINT32);
        }
        // Build the info for our shader
        let pc = PushConstants {
            view_proj: view_proj.to_cols_array_2d(),
            colour,
            opacity,
            line_width,
            grid_spacing,
        };

        // Push it out to the shader
        unsafe {
            dev.cmd_push_constants(
                cb,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                bytemuck::bytes_of(&pc),
            );

            dev.cmd_draw_indexed(cb, self.index_count, 1, 0, 0, 0);
            dev.cmd_end_render_pass(cb);
            dev.end_command_buffer(cb)?;

            Ok(cb)
        }
    }
}

impl Drop for ChaperoneRenderer {
    fn drop(&mut self) {
        unsafe {
            let d = &self.device;
            d.free_command_buffers(self.cmd_pool, &self.cmd_bufs);
            d.destroy_command_pool(self.cmd_pool, None);
            d.destroy_buffer(self.vertex_buf, None);
            d.free_memory(self.vertex_mem, None);
            d.destroy_buffer(self.index_buf, None);
            d.free_memory(self.index_mem, None);
            d.destroy_image_view(self.depth_view, None);
            d.destroy_image(self.depth_image, None);
            d.free_memory(self.depth_mem, None);
            d.destroy_pipeline(self.pipeline, None);
            d.destroy_pipeline_layout(self.pipeline_layout, None);
            d.destroy_render_pass(self.render_pass, None);
        }
    }
}

/// This is just for pushing a buffer up to Vulkan for handling
unsafe fn upload_buffer(
    device: &ash::Device,
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    usage: vk::BufferUsageFlags,
    data: &[u8],
) -> Result<(vk::Buffer, vk::DeviceMemory)> {
    let buf = unsafe {
        device.create_buffer(
            &vk::BufferCreateInfo::default()
                .size(data.len() as u64)
                .usage(usage)
                .sharing_mode(vk::SharingMode::EXCLUSIVE),
            None,
        )
    }?;
    let req = unsafe { device.get_buffer_memory_requirements(buf) };
    let mem_idx = find_memory_type(
        mem_props,
        req.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )?;

    let mem = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(req.size)
                .memory_type_index(mem_idx),
            None,
        )
    }?;
    unsafe { device.bind_buffer_memory(buf, mem, 0) }?;

    let ptr = unsafe { device.map_memory(mem, 0, req.size, vk::MemoryMapFlags::empty()) }?;
    unsafe {
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
        device.unmap_memory(mem)
    };

    Ok((buf, mem))
}
