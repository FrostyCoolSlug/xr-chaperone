use std::sync::Arc;

use anyhow::{Context, Result};
use ash::vk;
use ash::vk::Handle;
use glam::Vec2;
use openxr as xr;
use parking_lot::Mutex;
use tracing::{info, warn};

use crate::app_state::{AppState, Phase, XRState};
use crate::boundary;
use crate::config::Config;
use crate::mesh;
use crate::renderer::{ChaperoneRenderer, EyeSwapChain};
use crate::xr_session::{VulkanContext, XrContext, projection_from_fov, view_from_pose};

const NEAR: f32 = 0.05;
const FAR: f32 = 100.0;

pub fn run_xr_thread(state: Arc<Mutex<AppState>>, cfg: Config) {
    if let Err(e) = xr_main(state.clone(), cfg) {
        if state.lock().xr_state != XRState::Running {
            state.lock().xr_state = XRState::Error(e.to_string());
        }

        warn!("XR thread error: {e:#}");
        state.lock().xr_exit_requested = true;
    }
}

fn xr_main(state: Arc<Mutex<AppState>>, mut cfg: Config) -> Result<()> {
    let xr_entry = unsafe { xr::Entry::load() }.context("OpenXR loader not found")?;

    // Make sure the extensions we need are available in the XR Runtime, else bail.
    let available = xr_entry.enumerate_extensions()?;
    anyhow::ensure!(
        available.khr_vulkan_enable2,
        "OpenXR runtime does not support XR_KHR_vulkan_enable2"
    );
    anyhow::ensure!(
        available.extx_overlay,
        "OpenXR runtime does not support XR_EXTX_overlay"
    );

    let mut exts = xr::ExtensionSet::default();
    exts.khr_vulkan_enable2 = true;
    exts.extx_overlay = true;

    // Create the Chaperone Instance and naming with the extension
    let xr_instance = xr_entry.create_instance(
        &xr::ApplicationInfo {
            application_name: "xr-chaperone",
            application_version: 1,
            engine_name: "none",
            engine_version: 0,
            api_version: xr::Version::new(1, 0, 0),
        },
        &exts,
        &[],
    )?;

    let system_id = xr_instance
        .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
        .context("No HMD found")?;

    // Must be called before xrCreateSession AND before xrCreateVulkanDeviceKHR
    let reqs = xr_instance.graphics_requirements::<xr::Vulkan>(system_id)?;
    info!(
        "Vulkan requirements: min={}.{} max={}.{}",
        reqs.min_api_version_supported.major(),
        reqs.min_api_version_supported.minor(),
        reqs.max_api_version_supported.major(),
        reqs.max_api_version_supported.minor(),
    );

    // Create the Vulkan and XR contexts
    let vk = unsafe { VulkanContext::new(&xr_instance, system_id)? };
    let mut xr = XrContext::new(xr_instance, system_id, &vk)?;
    info!("XR + Vulkan session ready");

    let supported_fmts = xr.session.enumerate_swapchain_formats()?;
    let sc_format = [
        vk::Format::R8G8B8A8_UNORM,
        vk::Format::B8G8R8A8_UNORM,
        vk::Format::R8G8B8A8_SRGB,
        vk::Format::B8G8R8A8_SRGB,
    ]
    .iter()
    .find(|&&f| supported_fmts.contains(&(f.as_raw() as u32)))
    .copied()
    .context("No supported swapchain format")?;

    let view_count = xr.view_config.len();
    let mut xr_swap_chains: Vec<xr::Swapchain<xr::Vulkan>> = Vec::with_capacity(view_count);
    let mut eye_swap_chains: Vec<EyeSwapChain> = Vec::with_capacity(view_count);
    for vc in &xr.view_config {
        let w = vc.recommended_image_rect_width;
        let h = vc.recommended_image_rect_height;

        let sc = xr.session.create_swapchain(&xr::SwapchainCreateInfo {
            create_flags: xr::SwapchainCreateFlags::EMPTY,
            usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT
                | xr::SwapchainUsageFlags::SAMPLED,
            format: sc_format.as_raw() as u32, // u32 in openxr 0.19
            sample_count: 1,
            width: w,
            height: h,
            face_count: 1,
            array_size: 1,
            mip_count: 1,
        })?;

        let images: Vec<vk::Image> = sc
            .enumerate_images()?
            .into_iter()
            .map(vk::Image::from_raw)
            .collect();

        let image_views = images
            .iter()
            .map(|&img| unsafe {
                vk.device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(img)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(sc_format)
                        .subresource_range(vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: 0,
                            layer_count: 1,
                        }),
                    None,
                )
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;

        xr_swap_chains.push(sc);
        eye_swap_chains.push(EyeSwapChain {
            image_views,
            framebuffers: vec![],
            format: sc_format,
            extent: vk::Extent2D {
                width: w,
                height: h,
            },
        });
    }

    // Ok, we want OPAQUE here, at least for now, we don't need to blend with the real world
    // (at least, yet, AR might be something to look into later)
    let preferred_blend_mode = xr::EnvironmentBlendMode::OPAQUE;

    // Build the initial renderers and framebuffer from config
    let mut current_polygon: Vec<Vec2> = cfg.polygon();
    let mut renderers = build_renderers(&vk, &eye_swap_chains, &current_polygon, &cfg)?;
    rebuild_framebuffers(&vk, &mut eye_swap_chains, &renderers)?;

    let mut session_running = false;
    let mut was_trigger_pressed = false;

    // Before we fire off here, set our state to 'Running'
    state.lock().xr_state = XRState::Running;

    // Run the main rendering loop
    'main: loop {
        // Firstly, check whether the UI has requested an exit, and if so, break out
        if state.lock().ui_exit_requested {
            break 'main;
        }

        let mut buf = xr::EventDataBuffer::new();
        while let Some(event) = xr.instance.poll_event(&mut buf)? {
            use xr::Event::*;
            match event {
                SessionStateChanged(e) => {
                    info!("XR state → {:?}", e.state());
                    match e.state() {
                        xr::SessionState::READY => {
                            xr.session
                                .begin(xr::ViewConfigurationType::PRIMARY_STEREO)?;
                            session_running = true;
                        }
                        xr::SessionState::STOPPING => {
                            xr.session.end()?;
                            session_running = false;
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                            // We're on our way out, let the Upstream UI know to Quit
                            state.lock().xr_exit_requested = true;
                            break 'main;
                        }
                        _ => {}
                    }
                }
                InstanceLossPending(_) => break 'main,
                _ => {}
            }
        }

        if !session_running {
            std::thread::sleep(std::time::Duration::from_millis(5));
            continue;
        }

        let frame_state = xr.frame_waiter.wait()?;
        xr.frame_stream.begin()?;

        if !frame_state.should_render {
            xr.frame_stream.end(
                frame_state.predicted_display_time,
                preferred_blend_mode,
                &[],
            )?;
            continue;
        }

        let display_time = frame_state.predicted_display_time;
        let phase = state.lock().phase.clone();

        match phase {
            Phase::Unconfigured | Phase::Review => {
                xr.frame_stream
                    .end(display_time, preferred_blend_mode, &[])?;
            }

            Phase::Drawing => {
                // Controller position is updated here so the UI thread can render a live
                // preview of what the user is actively configuring.
                update_positions_and_controllers(
                    &xr,
                    &state,
                    display_time,
                    true,
                    &mut was_trigger_pressed,
                );
                xr.frame_stream
                    .end(display_time, preferred_blend_mode, &[])?;
            }

            Phase::Active => {
                // Pick up any settings changes posted by the UI.
                // Only rebuild the mesh/renderer if the geometry changed.
                // Everything else is handled in record().
                // Keep this scoped, so we don't need the lock longer than needed
                let mut s = state.lock();
                let new_cfg = s.pending_config.take();
                let new_polygon = s.polygon.clone();
                drop(s);

                let mut needs_rebuild = false;

                if let Some(new_cfg) = new_cfg {
                    // Check whether values have changed which would denote a rebuild
                    needs_rebuild = new_cfg.wall_height != cfg.wall_height
                        || new_cfg.grid_spacing != cfg.grid_spacing;

                    // TODO: Check if line width be needed here

                    cfg = new_cfg;
                }

                // Check whether the polygon has changed, which would also denote a rebuild
                if new_polygon != current_polygon && new_polygon.len() >= 3 {
                    current_polygon = new_polygon;
                    needs_rebuild = true;
                }

                if needs_rebuild {
                    rebuild_renderer_pipeline(
                        &vk,
                        &mut eye_swap_chains,
                        &mut renderers,
                        &current_polygon,
                        &cfg,
                    )?;
                }

                // Now we update the controller and headset positions
                update_positions_and_controllers(
                    &xr,
                    &state,
                    display_time,
                    false,
                    &mut was_trigger_pressed,
                );

                // Collect the positions of all tracked items
                // TODO: Check other devices? (Ex: Tracking Pucks)
                let proximity_positions: Vec<glam::Vec3> =
                    [&xr.head_space, &xr.right_space, &xr.left_space]
                        .iter()
                        .filter_map(|space| space.locate(&xr.stage, display_time).ok())
                        .map(|loc| {
                            let p = loc.pose.position;
                            glam::Vec3::new(p.x, p.y, p.z)
                        })
                        .collect();

                // Determine opacity based on the items proximity to the boundary
                let opacity = boundary::max_visibility(
                    &current_polygon,
                    &proximity_positions,
                    cfg.fade_start,
                    cfg.fade_end,
                );

                let (_flags, views) = xr.session.locate_views(
                    xr::ViewConfigurationType::PRIMARY_STEREO,
                    display_time,
                    &xr.stage,
                )?;

                // Ok, complicated rendery bit...
                // For each eye, acquire a fresh swapchain image, render the chaperone mesh into it,
                // then release it back to the XR runtime ready for composition.
                for (eye_idx, view) in views.iter().enumerate() {
                    let eye_sc = &eye_swap_chains[eye_idx];
                    let rend = &renderers[eye_idx];

                    // Borrow the next available image from the XR runtime's swapchain
                    let img_idx = xr_swap_chains[eye_idx].acquire_image()? as usize;
                    xr_swap_chains[eye_idx].wait_image(xr::Duration::INFINITE)?;

                    // Build the model-view-projection matrix for this eye's pose and field of view
                    let mvp =
                        projection_from_fov(&view.fov, NEAR, FAR) * view_from_pose(&view.pose);

                    // Record the draw commands for the chaperone mesh into a command buffer
                    let cb = unsafe {
                        rend.record(
                            img_idx,
                            eye_sc.framebuffers[img_idx],
                            eye_sc.extent,
                            mvp,
                            opacity,
                            cfg.grid_colour,
                            cfg.line_width,
                            cfg.grid_spacing,
                        )?
                    };

                    // Submit the command buffer to the GPU and wait for it to finish rendering
                    let submit =
                        vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cb));
                    unsafe {
                        vk.device
                            .queue_submit(vk.queue, &[submit], vk::Fence::null())?;
                        vk.device.queue_wait_idle(vk.queue)?;
                    }

                    // Return the rendered image to the XR runtime, ready to be composited
                    xr_swap_chains[eye_idx].release_image()?;
                }

                // Build the projection views that tell the compositor where each eye's rendered image
                // sits in 3D space, and which part of the swapchain image to sample from.
                let proj_views: Vec<_> = views
                    .iter()
                    .enumerate()
                    .map(|(eye_idx, view)| {
                        let eye_sc = &eye_swap_chains[eye_idx];
                        xr::CompositionLayerProjectionView::new()
                            .pose(view.pose)
                            .fov(view.fov)
                            .sub_image(
                                xr::SwapchainSubImage::new()
                                    .swapchain(&xr_swap_chains[eye_idx])
                                    .image_array_index(0)
                                    .image_rect(xr::Rect2Di {
                                        offset: xr::Offset2Di { x: 0, y: 0 },
                                        extent: xr::Extent2Di {
                                            width: eye_sc.extent.width as i32,
                                            height: eye_sc.extent.height as i32,
                                        },
                                    }),
                            )
                    })
                    .collect();

                // Wrap the projection views in a composition layer, with alpha blending flags so the
                // compositor correctly blends the chaperone overlay on top of the headset's view.
                let layer = xr::CompositionLayerProjection::new()
                    .layer_flags(
                        xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA
                            | xr::CompositionLayerFlags::UNPREMULTIPLIED_ALPHA,
                    )
                    .space(&xr.stage)
                    .views(&proj_views);

                // Submit the frame to the XR runtime. If opacity is effectively zero there is nothing
                // to show, so we skip the layer entirely rather than compositing a transparent image.
                if opacity > 0.001 {
                    xr.frame_stream
                        .end(display_time, preferred_blend_mode, &[&layer])?;
                } else {
                    xr.frame_stream
                        .end(display_time, preferred_blend_mode, &[])?;
                }
            }
        }
    }

    info!("XR thread exiting.");
    Ok(())
}

fn build_renderers(
    vk: &VulkanContext,
    eye_swap_chains: &[EyeSwapChain],
    polygon: &[Vec2],
    cfg: &Config,
) -> Result<Vec<ChaperoneRenderer>> {
    let mesh = mesh::build_mesh(polygon, cfg.wall_height, cfg.grid_spacing);
    eye_swap_chains
        .iter()
        .map(|sc| unsafe {
            ChaperoneRenderer::new(
                &vk.instance,
                vk.physical_device,
                vk.device.clone(),
                vk.queue_family,
                &mesh,
                sc,
            )
        })
        .collect()
}

fn destroy_framebuffers(vk: &VulkanContext, eye_swapchains: &mut [EyeSwapChain]) {
    for eye_sc in eye_swapchains.iter_mut() {
        for fb in eye_sc.framebuffers.drain(..) {
            unsafe {
                vk.device.destroy_framebuffer(fb, None);
            }
        }
    }
}

fn rebuild_framebuffers(
    vk: &VulkanContext,
    eye_swap_chains: &mut [EyeSwapChain],
    renderers: &[ChaperoneRenderer],
) -> Result<()> {
    for (eye_sc, renderer) in eye_swap_chains.iter_mut().zip(renderers.iter()) {
        let mut framebuffers = Vec::with_capacity(eye_sc.image_views.len());
        for &view in &eye_sc.image_views {
            let fb = unsafe {
                vk.device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(renderer.render_pass)
                        .attachments(&[view, renderer.depth_view])
                        .width(eye_sc.extent.width)
                        .height(eye_sc.extent.height)
                        .layers(1),
                    None,
                )?
            };
            framebuffers.push(fb);
        }
        eye_sc.framebuffers = framebuffers;
    }
    Ok(())
}

/// Tears down and reconstructs the full renderer pipeline. Called whenever the polygon
/// or a geometry-affecting config value (wall_height, grid_spacing) changes.
fn rebuild_renderer_pipeline(
    vk: &VulkanContext,
    eye_swap_chains: &mut [EyeSwapChain],
    renderers: &mut Vec<ChaperoneRenderer>,
    polygon: &[Vec2],
    cfg: &Config,
) -> Result<()> {
    destroy_framebuffers(vk, eye_swap_chains);
    renderers.clear(); // Drop all renderers, triggering Drop cleanup
    *renderers = build_renderers(vk, eye_swap_chains, polygon, cfg)?;
    rebuild_framebuffers(vk, eye_swap_chains, renderers)?;
    Ok(())
}

/// Updates headset/controller positions and handles tracing logic.
fn update_positions_and_controllers(
    xr: &XrContext,
    state: &Arc<Mutex<AppState>>,
    display_time: xr::Time,
    tracing: bool,
    was_trigger_pressed: &mut bool,
) {
    xr.session
        .sync_actions(&[xr::ActiveActionSet::new(&xr.action_set)])
        .ok();

    // Locate all tracked spaces before taking the state lock
    let right_loc = xr.right_space.locate(&xr.stage, display_time).ok();
    let left_loc = xr.left_space.locate(&xr.stage, display_time).ok();
    let head_loc = xr.head_space.locate(&xr.stage, display_time).ok();

    let right_trigger = xr
        .right_trigger
        .state(&xr.session, xr::Path::NULL)
        .map(|s| s.current_state)
        .unwrap_or(0.0);

    let left_trigger = xr
        .left_trigger
        .state(&xr.session, xr::Path::NULL)
        .map(|s| s.current_state)
        .unwrap_or(0.0);

    let mut s = state.lock();

    // Always update the positional data
    s.right_controller_pos = right_loc.map(|loc| loc.pose);
    s.left_controller_pos = left_loc.map(|loc| loc.pose);
    s.headset_pos = head_loc.map(|loc| loc.pose);

    if tracing {
        // If we're tracing, check button presses and behaviours
        let right_pressed = right_trigger > 0.5;
        let left_pressed = left_trigger > 0.5;
        let either_pressed = right_pressed || left_pressed;

        if either_pressed && !*was_trigger_pressed {
            let trace_pos = if right_pressed {
                s.right_controller_pos.or(s.left_controller_pos)
            } else {
                s.left_controller_pos.or(s.right_controller_pos)
            };

            if let Some(pos) = trace_pos {
                s.push_trace_point(Vec2 {
                    x: pos.position.x,
                    y: pos.position.z,
                });
            }
        }
        *was_trigger_pressed = either_pressed;
    }
}
