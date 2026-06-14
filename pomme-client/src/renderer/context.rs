use std::ffi::{CStr, c_char};
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
#[cfg(debug_assertions)]
use pyronyx::ext::debug_utils::DebugUtilsInstance;
use pyronyx::khr::surface::{SurfaceInstance, SurfacePhysicalDevice};
use pyronyx::raw_window_handle::{create_surface, get_required_extensions};
use pyronyx::{khr, vk};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use thiserror::Error;
use winit::window::Window;

use super::MAX_FRAMES_IN_FLIGHT;

#[derive(Error, Debug)]
pub enum ContextError {
    #[error("Vulkan error: {0}")]
    Vulkan(#[from] vk::Error),

    #[error("no suitable GPU found")]
    NoSuitableGpu,

    #[error("allocator error: {0}")]
    Allocator(#[from] pomme_gpu_allocator::AllocationError),

    #[error("surface error: {0}")]
    HandleError(#[from] raw_window_handle::HandleError),
}

const VK_APP_NAME: &CStr = c"Pomme";
const VK_APP_VERSION: u32 = vk::make_api_version(0, 0, 1, 0);
const VK_ENGINE_NAME: &CStr = c"Pomme Engine";
const VK_ENGINE_VERSION: u32 = vk::make_api_version(0, 0, 1, 0);
const VK_API_VERSION: u32 = vk::API_VERSION_1_4;

#[cfg(debug_assertions)]
const VALIDATION_LAYERS: &[&CStr] = &[c"VK_LAYER_KHRONOS_validation"];

const DEVICE_EXTENSIONS: &[&CStr] = &[
    khr::swapchain::NAME,
    #[cfg(target_os = "macos")]
    khr::portability_subset::NAME,
];

pub struct VulkanContext {
    pub instance: vk::Instance,
    pub surface: vk::SurfaceKHR,
    pub allocator: ManuallyDrop<Arc<Mutex<Allocator>>>,

    pub physical_device: vk::PhysicalDevice,
    pub device: vk::Device,

    pub graphics_queue: vk::Queue,
    pub graphics_family: u32,
    pub present_queue: vk::Queue,
    pub present_family: u32,

    pub command_pool: vk::CommandPool,
    pub command_buffers: [vk::CommandBuffer; MAX_FRAMES_IN_FLIGHT],

    pub image_available_semaphores: [vk::Semaphore; MAX_FRAMES_IN_FLIGHT],
    pub in_flight_fences: [vk::Fence; MAX_FRAMES_IN_FLIGHT],
    pub frame_index: usize,

    #[cfg(debug_assertions)]
    debug_messenger: vk::DebugUtilsMessengerEXT,

    pub gpu_name: String,
    pub vulkan_version: String,
}

impl VulkanContext {
    pub fn new(window: &Window) -> Result<Self, ContextError> {
        let display_handle = window.display_handle()?.as_raw();
        let window_handle = window.window_handle()?.as_raw();

        #[allow(unused_mut)]
        let mut extensions = get_required_extensions(display_handle)?.to_vec();

        #[cfg(target_os = "macos")]
        extensions.push(khr::portability_enumeration::NAME.as_ptr());

        #[cfg(debug_assertions)]
        let validation_available = check_validation_layer_support();

        #[cfg(debug_assertions)]
        if validation_available {
            use pyronyx::ext;

            extensions.push(ext::debug_utils::NAME.as_ptr());
        }

        #[cfg(debug_assertions)]
        let layers: &[&CStr] = if validation_available {
            VALIDATION_LAYERS
        } else {
            tracing::warn!(
                "Vulkan validation layers not available - install the Vulkan SDK for debug diagnostics"
            );
            &[]
        };

        #[cfg(not(debug_assertions))]
        let layers: &[&CStr] = &[];

        let layer_names: Vec<*const c_char> = layers.iter().map(|layer| layer.as_ptr()).collect();

        let app_info = vk::ApplicationInfo {
            application_name: VK_APP_NAME.as_ptr(),
            application_version: VK_APP_VERSION,
            engine_name: VK_ENGINE_NAME.as_ptr(),
            engine_version: VK_ENGINE_VERSION,
            api_version: VK_API_VERSION,
            ..Default::default()
        };

        let instance_info = vk::InstanceCreateInfo {
            application_info: &app_info,
            enabled_extension_count: extensions.len() as u32,
            enabled_extension_names: extensions.as_ptr(),

            #[cfg(target_os = "macos")]
            flags: vk::InstanceCreateFlags::EnumeratePortabilityKHR,

            enabled_layer_count: layer_names.len() as u32,
            enabled_layer_names: layer_names.as_ptr(),
            ..Default::default()
        };

        #[cfg(debug_assertions)]
        let mut debug_create_info = populate_debug_messenger_create_info();

        #[cfg(debug_assertions)]
        let instance_info = instance_info.next(&mut debug_create_info);

        let instance = unsafe { vk::Instance::create(&instance_info, None)? };

        #[cfg(debug_assertions)]
        let debug_messenger = if validation_available {
            instance.create_debug_utils_messenger(&populate_debug_messenger_create_info(), None)?
        } else {
            vk::DebugUtilsMessengerEXT::null()
        };

        let surface = create_surface(&instance, display_handle, window_handle)?;

        let (physical_device, graphics_family, present_family) =
            pick_physical_device(&instance, surface)?;

        let (gpu_name, vulkan_version) = {
            let props = physical_device.get_properties();
            let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }
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
        tracing::info!("GPU: {gpu_name} ({vulkan_version})");

        let queue_priority = 1.0f32;

        let queue_create_infos: &[_] = if graphics_family == present_family {
            &[vk::DeviceQueueCreateInfo {
                queue_family_index: graphics_family,
                queue_priorities: &queue_priority,
                queue_count: 1,
                ..Default::default()
            }]
        } else {
            &[
                vk::DeviceQueueCreateInfo {
                    queue_family_index: graphics_family,
                    queue_priorities: &queue_priority,
                    queue_count: 1,
                    ..Default::default()
                },
                vk::DeviceQueueCreateInfo {
                    queue_family_index: present_family,
                    queue_priorities: &queue_priority,
                    queue_count: 1,
                    ..Default::default()
                },
            ]
        };

        let mut vk12_features = vk::PhysicalDeviceVulkan12Features {
            // MoltenVK lacks drawIndirectCount; the chunk renderer has a macOS fallback.
            draw_indirect_count: if cfg!(target_os = "macos") {
                vk::FALSE
            } else {
                vk::TRUE
            },
            ..Default::default()
        };

        let device_extension_names: Vec<*const c_char> =
            DEVICE_EXTENSIONS.iter().map(|ext| ext.as_ptr()).collect();

        let device_info = vk::DeviceCreateInfo {
            queue_create_info_count: queue_create_infos.len() as u32,
            queue_create_infos: queue_create_infos.as_ptr(),
            enabled_extension_count: device_extension_names.len() as u32,
            enabled_extension_names: device_extension_names.as_ptr(),
            ..Default::default()
        }
        .next(&mut vk12_features);

        let device = unsafe { physical_device.create_device(&device_info, None, &instance)? };

        let graphics_queue = unsafe { device.get_device_queue(graphics_family, 0) };
        let present_queue = unsafe { device.get_device_queue(present_family, 0) };

        let allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.clone(),
            device: device.clone(),
            physical_device,
            debug_settings: Default::default(),
            buffer_device_address: false,
            allocation_sizes: Default::default(),
        })?;
        let allocator = ManuallyDrop::new(Arc::new(Mutex::new(allocator)));

        let pool_info = vk::CommandPoolCreateInfo {
            flags: vk::CommandPoolCreateFlags::ResetCommandBuffer,
            queue_family_index: graphics_family,
            ..Default::default()
        };
        let command_pool = device.create_command_pool(&pool_info, None)?;

        let alloc_info = vk::CommandBufferAllocateInfo {
            command_pool,
            level: vk::CommandBufferLevel::Primary,
            command_buffer_count: MAX_FRAMES_IN_FLIGHT as u32,
            ..Default::default()
        };
        let mut command_buffers = [vk::CommandBuffer::default(); MAX_FRAMES_IN_FLIGHT];
        unsafe { device.allocate_command_buffers(&alloc_info, &mut command_buffers)? };

        let sem_info = vk::SemaphoreCreateInfo::default();
        let fence_info = vk::FenceCreateInfo {
            flags: vk::FenceCreateFlags::Signaled,
            ..Default::default()
        };

        let mut image_available_semaphores = [vk::Semaphore::default(); MAX_FRAMES_IN_FLIGHT];
        let mut in_flight_fences = [vk::Fence::default(); MAX_FRAMES_IN_FLIGHT];

        for i in 0..MAX_FRAMES_IN_FLIGHT {
            image_available_semaphores[i] = device.create_semaphore(&sem_info, None)?;
            in_flight_fences[i] = device.create_fence(&fence_info, None)?;
        }

        Ok(Self {
            instance,
            surface,
            allocator,
            physical_device,
            device,
            graphics_queue,
            graphics_family,
            present_queue,
            present_family,
            command_pool,
            command_buffers,
            image_available_semaphores,
            in_flight_fences,
            frame_index: 0,
            #[cfg(debug_assertions)]
            debug_messenger,
            gpu_name,
            vulkan_version,
        })
    }

    pub fn advance_frame(&mut self) {
        self.frame_index = (self.frame_index + 1) % MAX_FRAMES_IN_FLIGHT;
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.wait_idle();

            for &fence in &self.in_flight_fences {
                self.device.destroy_fence(fence, None);
            }
            for &sem in &self.image_available_semaphores {
                self.device.destroy_semaphore(sem, None);
            }

            self.device.destroy_command_pool(self.command_pool, None);

            ManuallyDrop::drop(&mut self.allocator);

            self.device.destroy(None);

            #[cfg(debug_assertions)]
            if self.debug_messenger != vk::DebugUtilsMessengerEXT::null() {
                self.instance
                    .destroy_debug_utils_messenger(self.debug_messenger, None);
            }

            self.instance.destroy_surface(self.surface, None);
            self.instance.destroy(None);
        }
    }
}

fn pick_physical_device(
    instance: &vk::Instance,
    surface: vk::SurfaceKHR,
) -> Result<(vk::PhysicalDevice, u32, u32), ContextError> {
    let devices = unsafe { instance.enumerate_physical_devices()? };

    let mut candidates: Vec<_> = devices
        .into_iter()
        .filter_map(|pd| {
            let (gf, pf) = find_queue_families(&pd, surface)?;
            if !supports_required_extensions(&pd) {
                return None;
            }
            let props = pd.get_properties();
            let score = match props.device_type {
                vk::PhysicalDeviceType::DiscreteGpu => 100,
                vk::PhysicalDeviceType::IntegratedGpu => 50,
                _ => 10,
            };
            Some((pd, gf, pf, score))
        })
        .collect();

    candidates.sort_by_key(|c| std::cmp::Reverse(c.3));

    candidates
        .into_iter()
        .next()
        .map(|(pd, gf, pf, _)| (pd, gf, pf))
        .ok_or(ContextError::NoSuitableGpu)
}

fn supports_required_extensions(device: &vk::PhysicalDevice) -> bool {
    let available = device
        .enumerate_device_extension_properties(None)
        .unwrap_or_default();
    DEVICE_EXTENSIONS.iter().all(|&required| {
        available
            .iter()
            .any(|ext| unsafe { CStr::from_ptr(ext.extension_name.as_ptr()) == required })
    })
}

fn find_queue_families(device: &vk::PhysicalDevice, surface: vk::SurfaceKHR) -> Option<(u32, u32)> {
    let families = device.get_queue_family_properties();

    let mut graphics = None;
    let mut present = None;

    for (i, family) in families.iter().enumerate() {
        let i = i as u32;

        if family.queue_flags.contains(vk::QueueFlags::Graphics) {
            graphics = Some(i);
        }

        if device.get_surface_support(i, surface).unwrap_or(false) {
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

#[cfg(debug_assertions)]
fn check_validation_layer_support() -> bool {
    let available = vk::enumerate_instance_layer_properties().unwrap_or_default();
    VALIDATION_LAYERS.iter().all(|&layer| {
        available.iter().any(|props| {
            let name = unsafe { CStr::from_ptr(props.layer_name.as_ptr()) };
            name == layer
        })
    })
}

#[cfg(debug_assertions)]
fn populate_debug_messenger_create_info() -> vk::DebugUtilsMessengerCreateInfoEXT<'static> {
    vk::DebugUtilsMessengerCreateInfoEXT {
        message_severity: vk::DebugUtilsMessageSeverityFlagsEXT::Error
            | vk::DebugUtilsMessageSeverityFlagsEXT::Warning,
        message_type: vk::DebugUtilsMessageTypeFlagsEXT::General
            | vk::DebugUtilsMessageTypeFlagsEXT::Validation
            | vk::DebugUtilsMessageTypeFlagsEXT::Performance,
        pfn_user_callback: Some(vulkan_debug_callback),
        ..Default::default()
    }
}

#[cfg(debug_assertions)]
extern "system" fn vulkan_debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _ty: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _user_data: *mut std::ffi::c_void,
) -> u32 {
    let msg = unsafe { CStr::from_ptr((*data).message) }.to_string_lossy();
    match severity {
        vk::DebugUtilsMessageSeverityFlagsEXT::Error => tracing::error!("[Vulkan] {msg}"),
        vk::DebugUtilsMessageSeverityFlagsEXT::Warning => tracing::warn!("[Vulkan] {msg}"),
        _ => tracing::debug!("[Vulkan] {msg}"),
    }
    0
}
