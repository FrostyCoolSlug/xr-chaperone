use anyhow::{Context, Result};
use ash::vk;
use ash::vk::Handle;
use glam::{Mat4, Quat, Vec3};
use openxr as xr;
use openxr_sys as xr_sys;
use openxr_sys::Handle as xr_handle;

/// The Vulkan Context (duh)
pub struct VulkanContext {
    pub instance: ash::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue_family: u32,
    pub queue: vk::Queue,
}

impl VulkanContext {
    /// Create Vulkan handle driven by OpenXR
    pub unsafe fn new(xr_instance: &xr::Instance, system_id: xr::SystemId) -> Result<Self> {
        let entry = unsafe { ash::Entry::load().context("Could not load libvulkan") }?;

        type InstanceProcAddr = unsafe extern "system" fn(
            *const std::ffi::c_void,
            *const i8,
        )
            -> Option<unsafe extern "system" fn()>;

        let raw = entry.static_fn().get_instance_proc_addr;
        let instance_addr: InstanceProcAddr = unsafe { std::mem::transmute(raw) };

        let vk_instance_ptr = unsafe {
            xr_instance.create_vulkan_instance(
                system_id,
                instance_addr,
                &vk::InstanceCreateInfo::default() as *const _ as *const _,
            )
        }
        .context("xrCreateVulkanInstanceKHR")?
        .map_err(|e| anyhow::anyhow!("Vulkan instance create: {e:?}"))?;

        let vk_instance = unsafe {
            ash::Instance::load(
                entry.static_fn(),
                vk::Instance::from_raw(vk_instance_ptr as u64),
            )
        };

        let physical_device = vk::PhysicalDevice::from_raw(unsafe {
            xr_instance.vulkan_graphics_device(system_id, vk_instance_ptr)
        }? as u64);

        let queue_family =
            unsafe { vk_instance.get_physical_device_queue_family_properties(physical_device) }
                .into_iter()
                .enumerate()
                .find(|(_, p)| p.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .map(|(i, _)| i as u32)
                .context("No graphics queue family")?;

        let queue_info = vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&[1.0]);

        let vk_device_ptr = unsafe {
            xr_instance.create_vulkan_device(
                system_id,
                instance_addr,
                physical_device.as_raw() as *const u64 as _,
                &vk::DeviceCreateInfo::default()
                    .queue_create_infos(std::slice::from_ref(&queue_info))
                    as *const _ as *const _,
            )
        }
        .context("xrCreateVulkanDeviceKHR")?
        .map_err(|e| anyhow::anyhow!("Vulkan device create: {e:?}"))?;

        let device = unsafe {
            ash::Device::load(
                vk_instance.fp_v1_0(),
                vk::Device::from_raw(vk_device_ptr as u64),
            )
        };

        let queue = unsafe { device.get_device_queue(queue_family, 0) };

        Ok(Self {
            instance: vk_instance,
            physical_device,
            device,
            queue_family,
            queue,
        })
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

/// The XR Context
pub struct XrContext {
    pub instance: xr::Instance,
    pub session: xr::Session<xr::Vulkan>,
    pub frame_waiter: xr::FrameWaiter,
    pub frame_stream: xr::FrameStream<xr::Vulkan>,
    pub stage: xr::Space,
    pub view_config: Vec<xr::ViewConfigurationView>,

    pub action_set: xr::ActionSet,

    pub left_trigger: xr::Action<f32>,
    pub right_trigger: xr::Action<f32>,

    pub left_space: xr::Space,
    pub right_space: xr::Space,
    pub head_space: xr::Space,
}

impl XrContext {
    /// Create a new context, taking ownership of the instance
    pub fn new(
        instance: xr::Instance,
        system_id: xr::SystemId,
        vk: &VulkanContext,
    ) -> Result<Self> {
        let view_config = instance.enumerate_view_configuration_views(
            system_id,
            xr::ViewConfigurationType::PRIMARY_STEREO,
        )?;

        // Build the overlay info struct, We set the placement to 20 because
        // it needs to be on top of ALL other content, including potentially other
        // layers.
        let overlay_info = xr_sys::SessionCreateInfoOverlayEXTX {
            ty: xr_sys::StructureType::SESSION_CREATE_INFO_OVERLAY_EXTX,
            next: std::ptr::null(),
            create_flags: xr_sys::OverlaySessionCreateFlagsEXTX::EMPTY,
            session_layers_placement: 20,
        };

        // Graphics binding for Vulkan
        let graphics_binding = xr_sys::GraphicsBindingVulkanKHR {
            ty: xr_sys::StructureType::GRAPHICS_BINDING_VULKAN_KHR,
            next: &overlay_info as *const _ as *const _,
            instance: vk.instance.handle().as_raw() as _,
            physical_device: vk.physical_device.as_raw() as _,
            device: vk.device.handle().as_raw() as _,
            queue_family_index: vk.queue_family,
            queue_index: 0,
        };

        let session_create_info = xr_sys::SessionCreateInfo {
            ty: xr_sys::StructureType::SESSION_CREATE_INFO,
            next: &graphics_binding as *const _ as *const _,
            create_flags: xr_sys::SessionCreateFlags::EMPTY,
            system_id,
        };

        let mut session_handle = xr_sys::Session::NULL;
        unsafe {
            (instance.fp().create_session)(
                instance.as_raw(),
                &session_create_info,
                &mut session_handle,
            );
        }

        // Wrap the raw handle back into the safe types
        let (session, frame_waiter, frame_stream) = unsafe {
            xr::Session::<xr::Vulkan>::from_raw(instance.clone(), session_handle, Box::new(()))
        };

        let stage =
            session.create_reference_space(xr::ReferenceSpaceType::STAGE, xr::Posef::IDENTITY)?;
        let head_space =
            session.create_reference_space(xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)?;

        // TODO: I'm not confident about any of this code..
        // Ref: https://www.khronos.org/assets/uploads/developers/presentations/XR-Kaigi-Interaction-Only-with-notes_Dec20.pdf
        // From what I can tell reading the above document, simple_controller doesn't actually
        // support triggers, so requesting a trigger position will result in a breakage. For now,
        // we'll exclude that profile from the list. The 'correct' solution would be remapping it
        // to a button or something instead.
        //
        // Also, I need to find a cleaner way to handle this, it's a little verbose.
        let set = instance.create_action_set("chaperone", "Chaperone", 0)?;

        let left_pose = set.create_action::<xr::Posef>("left_grip", "Left grip", &[])?;
        let left_trigger = set.create_action::<f32>("left_trigger", "Left Trigger", &[])?;

        let right_pose = set.create_action::<xr::Posef>("right_grip", "Right grip", &[])?;
        let right_trigger = set.create_action::<f32>("right_trigger", "Right Trigger", &[])?;

        // To add 'Simple' support back, include:
        // "/interaction_profiles/khr/simple_controller",
        let profiles = [
            "/interaction_profiles/oculus/touch_controller",
            "/interaction_profiles/valve/index_controller",
            "/interaction_profiles/htc/vive_controller",
        ];
        for profile in &profiles {
            let path = instance.string_to_path(profile)?;
            let bindings = vec![
                xr::Binding::new(
                    &left_pose,
                    instance.string_to_path("/user/hand/left/input/grip/pose")?,
                ),
                xr::Binding::new(
                    &left_trigger,
                    instance.string_to_path("/user/hand/left/input/trigger/value")?,
                ),
                xr::Binding::new(
                    &right_pose,
                    instance.string_to_path("/user/hand/right/input/grip/pose")?,
                ),
                xr::Binding::new(
                    &right_trigger,
                    instance.string_to_path("/user/hand/right/input/trigger/value")?,
                ),
            ];
            let _ = instance.suggest_interaction_profile_bindings(path, &bindings);
        }

        session.attach_action_sets(&[&set])?;
        let left_space = left_pose.create_space(&session, xr::Path::NULL, xr::Posef::IDENTITY)?;
        let right_space = right_pose.create_space(&session, xr::Path::NULL, xr::Posef::IDENTITY)?;

        Ok(Self {
            instance,
            session,
            frame_waiter,
            frame_stream,
            stage,
            view_config,
            action_set: set,
            left_trigger,
            right_trigger,
            left_space,
            right_space,
            head_space,
        })
    }
}

// Construct a Frustum Projection Matrix from the FOV angles... Personal note, I fucking
// hate math. If you need to know how this works, it's something you can google :D
//
// Also, tell rustfmt to NOT reformat this, because it makes from_cols_array painful.
#[rustfmt::skip]
pub fn projection_from_fov(fov: &xr::Fovf, near: f32, far: f32) -> Mat4 {
    // Get the tangents from each direction
    let tan_left = fov.angle_left.tan();
    let tan_right = fov.angle_right.tan();
    let tan_up = fov.angle_up.tan();
    let tan_down = fov.angle_down.tan();

    // Calculate the width and height of the near plane based on the FOV angles
    let width = tan_right - tan_left;
    let height = tan_up - tan_down;

    // Some pre-calcs to try and keep the matrix clean :D
    let offset_x = (tan_right + tan_left) / width;
    let offset_y = (tan_up + tan_down) / height;
    let depth_z = -far / (far - near);
    let depth_w = -(far * near) / (far - near);

    // The main difference between this and OpenGL implementations is that Vulkan expects
    // the Y axis to be flipped, so we inverse it in grid [1,1]... Took me a fucking while
    // to find that one.
    Mat4::from_cols_array(&[
        2.0 / width,  0.0,           0.0,       0.0,
        0.0,         -2.0 / height,  0.0,       0.0,
        offset_x,     offset_y,      depth_z,  -1.0,
        0.0,          0.0,           depth_w,   0.0,
    ])
}

pub fn view_from_pose(pose: &xr::Posef) -> Mat4 {
    let position = Vec3::new(pose.position.x, pose.position.y, pose.position.z);
    let orientation = Quat::from_xyzw(
        pose.orientation.x,
        pose.orientation.y,
        pose.orientation.z,
        pose.orientation.w,
    );

    // Create a transformation matrix from rotation and translation,
    // then invert it to get the view matrix (camera transform)
    Mat4::from_rotation_translation(orientation, position).inverse()
}
