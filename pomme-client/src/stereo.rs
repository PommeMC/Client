//! The hook a head-mounted display runtime plugs into (`pomme-vr`'s OpenXR
//! backend): the renderer draws the world once per frame into its two-layer
//! images, one layer per eye, through a multiview pass. Raw Vulkan handles
//! and glam types only, so the client never links an XR runtime.

use std::ffi::{CString, c_void};

use glam::{Quat, Vec3};

/// A dispatchable Vulkan handle (`VkInstance`, `VkPhysicalDevice`, ...) as the
/// C API passes it.
pub type RawHandle = *const c_void;

/// Half-angles of an eye's view frustum in radians; `left` and `down` are
/// negative for a frustum that straddles the view axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fov {
    pub left: f32,
    pub right: f32,
    pub up: f32,
    pub down: f32,
}

/// One eye's pose in the seated reference space: metres from where the head
/// sat when the session started, looking down -Z with +Y up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeView {
    pub orientation: Quat,
    pub position: Vec3,
    pub fov: Fov,
}

/// The device the client created to the runtime's requirements.
pub struct StereoDevice {
    pub instance: RawHandle,
    pub physical_device: RawHandle,
    pub device: RawHandle,
    pub queue_family: u32,
    pub queue_index: u32,
    /// The Vulkan version the instance was created for, which the runtime
    /// must support.
    pub api_version: u32,
}

/// The runtime's swapchain: each `VkImage` holds both eyes as layers 0 and 1.
pub struct StereoImages {
    /// The `VkFormat` the runtime took from the client's preference list.
    pub format: i64,
    pub width: u32,
    pub height: u32,
    pub images: Vec<u64>,
}

/// A frame the runtime wants rendered, into `images[image_index]`.
pub struct StereoFrame {
    pub image_index: usize,
    pub eyes: [EyeView; 2],
}

pub trait StereoBackend {
    /// Instance extensions the runtime needs, ahead of instance creation.
    fn instance_extensions(&self) -> Result<Vec<CString>, String>;

    /// The GPU the runtime drives the headset from.
    fn physical_device(&self, instance: RawHandle) -> Result<RawHandle, String>;

    /// Device extensions the runtime needs, ahead of device creation.
    fn device_extensions(&self) -> Result<Vec<CString>, String>;

    /// Starts the session on the client's device, with a swapchain in the
    /// first of `formats` (`VkFormat`s, best first) the runtime supports.
    fn start(&mut self, device: StereoDevice, formats: &[i64]) -> Result<StereoImages, String>;

    /// Blocks until the runtime wants the next frame. `None` skips rendering
    /// this frame (the session isn't visible); `end_frame` still follows.
    fn begin_frame(&mut self) -> Option<StereoFrame>;

    /// Hands the frame back; `rendered` says whether `begin_frame`'s image was
    /// drawn and submitted.
    fn end_frame(&mut self, rendered: bool);

    /// The runtime asked the app to quit (headset session ending).
    fn should_exit(&self) -> bool;
}
