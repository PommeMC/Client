use std::ffi::{CStr, CString};
use std::sync::{Arc, Mutex};

use ash::ext::debug_utils;
use ash::khr::{surface, swapchain};
use ash::vk;
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use thiserror::Error;
use winit::window::Window;

use super::MAX_FRAMES_IN_FLIGHT;

#[derive(Error, Debug)]
pub enum ContextError {
    #[error("Vulkan error: {0}")]
    Vulkan(#[from] vk::Result),

    #[error("no suitable GPU found")]
    NoSuitableGpu,

    #[error("allocator error: {0}")]
    Allocator(#[from] gpu_allocator::AllocationError),

    #[error("surface error: {0}")]
    HandleError(#[from] raw_window_handle::HandleError),
}

#[allow(dead_code)]
pub struct VulkanContext {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    pub surface_loader: surface::Instance,
    pub surface: vk::SurfaceKHR,
    pub physical_device: vk::PhysicalDevice,
    pub device: ash::Device,
    pub graphics_queue: vk::Queue,
    pub present_queue: vk::Queue,
    pub graphics_family: u32,
    pub present_family: u32,
    pub swapchain_loader: swapchain::Device,
    pub allocator: std::mem::ManuallyDrop<Arc<Mutex<Allocator>>>,
    pub command_pool: vk::CommandPool,
    pub command_buffers: Vec<vk::CommandBuffer>,
    pub image_available: Vec<vk::Semaphore>,
    pub render_finished: Vec<vk::Semaphore>,
    pub in_flight_fences: Vec<vk::Fence>,
    pub frame_index: usize,
    pub gpu_name: String,
    pub vulkan_version: String,
    debug_messenger: Option<vk::DebugUtilsMessengerEXT>,
    debug_utils_loader: Option<debug_utils::Instance>,
}

impl VulkanContext {
    pub fn new(window: &Window) -> Result<Self, ContextError> {
        let entry = unsafe { ash::Entry::load().expect("failed to load Vulkan") };

        let app_info = vk::ApplicationInfo::default()
            .application_name(c"Pomme")
            .application_version(vk::make_api_version(0, 0, 1, 0))
            .engine_name(c"Pomme Engine")
            .engine_version(vk::make_api_version(0, 0, 1, 0))
            .api_version(vk::make_api_version(0, 1, 3, 0));

        let display_handle = window.display_handle()?.as_raw();
        let mut required_extensions =
            ash_window::enumerate_required_extensions(display_handle)?.to_vec();

        let validation_available = cfg!(debug_assertions)
            && unsafe { entry.enumerate_instance_layer_properties() }
                .unwrap_or_default()
                .iter()
                .any(|layer| {
                    let name = unsafe { CStr::from_ptr(layer.layer_name.as_ptr()) };
                    name.to_bytes() == b"VK_LAYER_KHRONOS_validation"
                });

        if validation_available {
            required_extensions.push(debug_utils::NAME.as_ptr());
        }

        let mut instance_flags = vk::InstanceCreateFlags::empty();
        if cfg!(target_os = "macos") {
            required_extensions.push(ash::khr::portability_enumeration::NAME.as_ptr());
            instance_flags |= vk::InstanceCreateFlags::ENUMERATE_PORTABILITY_KHR;
        }

        let layer_names: Vec<CString> = if validation_available {
            vec![CString::new("VK_LAYER_KHRONOS_validation").unwrap()]
        } else {
            if cfg!(debug_assertions) {
                tracing::warn!(
                    "Vulkan validation layers not available - install the Vulkan SDK for debug diagnostics"
                );
            }
            vec![]
        };
        let layer_ptrs: Vec<*const i8> = layer_names.iter().map(|l| l.as_ptr()).collect();

        let instance_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&required_extensions)
            .enabled_layer_names(&layer_ptrs)
            .flags(instance_flags);

        let instance = unsafe { entry.create_instance(&instance_info, None)? };

        let (debug_utils_loader, debug_messenger) = if validation_available {
            let loader = debug_utils::Instance::new(&entry, &instance);
            let messenger_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                        | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
                )
                .message_type(
                    vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                        | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                        | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                )
                .pfn_user_callback(Some(vulkan_debug_callback));

            let messenger = unsafe { loader.create_debug_utils_messenger(&messenger_info, None)? };
            (Some(loader), Some(messenger))
        } else {
            (None, None)
        };

        let surface_loader = surface::Instance::new(&entry, &instance);
        let surface = unsafe {
            ash_window::create_surface(
                &entry,
                &instance,
                display_handle,
                window.window_handle()?.as_raw(),
                None,
            )?
        };

        let (physical_device, graphics_family, present_family) =
            pick_physical_device(&instance, &surface_loader, surface)?;

        let (dev_name, vulkan_version) = unsafe {
            let props = instance.get_physical_device_properties(physical_device);
            let name = CStr::from_ptr(props.device_name.as_ptr())
                .to_string_lossy()
                .into_owned();
            let v = props.api_version;
            let ver = format!(
                "Vulkan {}.{}.{}",
                vk::api_version_major(v),
                vk::api_version_minor(v),
                vk::api_version_patch(v)
            );
            (name, ver)
        };
        tracing::info!("GPU: {dev_name} ({vulkan_version})");

        let unique_families: Vec<u32> = if graphics_family == present_family {
            vec![graphics_family]
        } else {
            vec![graphics_family, present_family]
        };

        let queue_priority = [1.0f32];
        let queue_infos: Vec<vk::DeviceQueueCreateInfo> = unique_families
            .iter()
            .map(|&family| {
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(family)
                    .queue_priorities(&queue_priority)
            })
            .collect();

        let mut device_extensions = vec![swapchain::NAME.as_ptr()];
        if cfg!(target_os = "macos") {
            device_extensions.push(ash::khr::portability_subset::NAME.as_ptr());
        }

        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&device_extensions);

        let device = unsafe { instance.create_device(physical_device, &device_info, None)? };

        let graphics_queue = unsafe { device.get_device_queue(graphics_family, 0) };
        let present_queue = unsafe { device.get_device_queue(present_family, 0) };

        let swapchain_loader = swapchain::Device::new(&instance, &device);

        let allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.clone(),
            device: device.clone(),
            physical_device,
            debug_settings: Default::default(),
            buffer_device_address: false,
            allocation_sizes: Default::default(),
        })?;
        let allocator = std::mem::ManuallyDrop::new(Arc::new(Mutex::new(allocator)));

        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(graphics_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let command_pool = unsafe { device.create_command_pool(&pool_info, None)? };

        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(MAX_FRAMES_IN_FLIGHT as u32);
        let command_buffers = unsafe { device.allocate_command_buffers(&alloc_info)? };

        let mut image_available = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut render_finished = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut in_flight_fences = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        let sem_info = vk::SemaphoreCreateInfo::default();
        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);

        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            unsafe {
                image_available.push(device.create_semaphore(&sem_info, None)?);
                render_finished.push(device.create_semaphore(&sem_info, None)?);
                in_flight_fences.push(device.create_fence(&fence_info, None)?);
            }
        }

        Ok(Self {
            entry,
            instance,
            surface_loader,
            surface,
            physical_device,
            device,
            graphics_queue,
            present_queue,
            graphics_family,
            present_family,
            swapchain_loader,
            allocator,
            command_pool,
            command_buffers,
            image_available,
            render_finished,
            in_flight_fences,
            gpu_name: dev_name,
            vulkan_version,
            frame_index: 0,
            debug_messenger,
            debug_utils_loader,
        })
    }

    pub fn advance_frame(&mut self) {
        self.frame_index = (self.frame_index + 1) % MAX_FRAMES_IN_FLIGHT;
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();

            for &fence in &self.in_flight_fences {
                self.device.destroy_fence(fence, None);
            }
            for &sem in &self.render_finished {
                self.device.destroy_semaphore(sem, None);
            }
            for &sem in &self.image_available {
                self.device.destroy_semaphore(sem, None);
            }

            self.device.destroy_command_pool(self.command_pool, None);

            drop(std::mem::ManuallyDrop::take(&mut self.allocator));

            self.device.destroy_device(None);
            self.surface_loader.destroy_surface(self.surface, None);

            if let (Some(loader), Some(messenger)) =
                (&self.debug_utils_loader, self.debug_messenger)
            {
                loader.destroy_debug_utils_messenger(messenger, None);
            }

            self.instance.destroy_instance(None);
        }
    }
}

fn pick_physical_device(
    instance: &ash::Instance,
    surface_loader: &surface::Instance,
    surface: vk::SurfaceKHR,
) -> Result<(vk::PhysicalDevice, u32, u32), ContextError> {
    let devices = unsafe { instance.enumerate_physical_devices()? };

    // Prefer discrete GPUs
    let mut candidates: Vec<_> = devices
        .into_iter()
        .filter_map(|pd| {
            let (gf, pf) = find_queue_families(instance, surface_loader, surface, pd)?;
            let props = unsafe { instance.get_physical_device_properties(pd) };
            let score = match props.device_type {
                vk::PhysicalDeviceType::DISCRETE_GPU => 100,
                vk::PhysicalDeviceType::INTEGRATED_GPU => 50,
                _ => 10,
            };
            Some((pd, gf, pf, score))
        })
        .collect();

    candidates.sort_by_key(|c| std::cmp::Reverse(c.3));

    candidates
        .first()
        .map(|&(pd, gf, pf, _)| (pd, gf, pf))
        .ok_or(ContextError::NoSuitableGpu)
}

fn find_queue_families(
    instance: &ash::Instance,
    surface_loader: &surface::Instance,
    surface: vk::SurfaceKHR,
    physical_device: vk::PhysicalDevice,
) -> Option<(u32, u32)> {
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };

    let mut graphics = None;
    let mut present = None;

    for (i, family) in families.iter().enumerate() {
        let i = i as u32;

        if family.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
            graphics = Some(i);
        }

        let present_support = unsafe {
            surface_loader
                .get_physical_device_surface_support(physical_device, i, surface)
                .unwrap_or(false)
        };
        if present_support {
            present = Some(i);
        }

        if graphics.is_some() && present.is_some() {
            break;
        }
    }

    match (graphics, present) {
        (Some(g), Some(p)) => Some((g, p)),
        _ => None,
    }
}

unsafe extern "system" fn vulkan_debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _ty: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _user_data: *mut std::ffi::c_void,
) -> vk::Bool32 {
    let msg = unsafe { CStr::from_ptr((*data).p_message) }.to_string_lossy();
    match severity {
        vk::DebugUtilsMessageSeverityFlagsEXT::ERROR => tracing::error!("[Vulkan] {msg}"),
        vk::DebugUtilsMessageSeverityFlagsEXT::WARNING => tracing::warn!("[Vulkan] {msg}"),
        _ => tracing::debug!("[Vulkan] {msg}"),
    }
    vk::FALSE
}
