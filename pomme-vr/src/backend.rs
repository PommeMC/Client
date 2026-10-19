//! `StereoBackend` over OpenXR's `XR_KHR_vulkan_enable`, where the client
//! creates the Vulkan instance and device itself to the runtime's
//! requirements (pyronyx can't wrap ones the runtime would create).

use std::ffi::CString;
use std::time::Duration;

use glam::{Quat, Vec3};
use openxr as xr;
use pomme_client::stereo::{
    EyeView, Fov, RawHandle, StereoBackend, StereoDevice, StereoFrame, StereoImages,
};

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;
/// Between polls while the headset isn't showing the app, so the loop
/// doesn't spin without `xrWaitFrame` to pace it.
const IDLE_POLL: Duration = Duration::from_millis(10);

pub struct OpenXrBackend {
    instance: xr::Instance,
    system: xr::SystemId,
    blend_mode: xr::EnvironmentBlendMode,
    session: Option<Session>,
    exit: bool,
}

/// What `start` creates on the client's device.
struct Session {
    session: xr::Session<xr::Vulkan>,
    frame_waiter: xr::FrameWaiter,
    frame_stream: xr::FrameStream<xr::Vulkan>,
    swapchain: xr::Swapchain<xr::Vulkan>,
    extent: xr::Extent2Di,
    /// The seated origin: where the head sat when the session began.
    space: xr::Space,
    running: bool,
    /// The frame `begin_frame` began: its display time, and whether it holds
    /// a swapchain image and the views it was rendered for.
    frame: Option<(xr::Time, Option<[xr::View; 2]>)>,
    last_eyes: Option<[EyeView; 2]>,
}

impl OpenXrBackend {
    pub fn new() -> Result<Self, String> {
        let entry = xr::Entry::linked(&()).map_err(|e| format!("OpenXR loader: {e}"))?;
        let available = entry
            .enumerate_extensions()
            .map_err(|e| format!("no OpenXR runtime: {e}"))?;
        if !available.khr_vulkan_enable {
            return Err("the OpenXR runtime has no Vulkan support".into());
        }
        let mut extensions = xr::ExtensionSet::default();
        extensions.khr_vulkan_enable = true;
        let instance = entry
            .create_instance(
                &xr::ApplicationInfo {
                    application_name: "Pomme",
                    engine_name: "Pomme",
                    api_version: xr::Version::new(1, 0, 0),
                    ..Default::default()
                },
                &extensions,
                &[],
                &(),
            )
            .map_err(|e| format!("OpenXR instance: {e}"))?;
        let system = instance
            .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
            .map_err(|e| format!("no headset: {e}"))?;
        let blend_mode = instance
            .enumerate_environment_blend_modes(system, VIEW_TYPE)
            .map_err(|e| e.to_string())?
            .first()
            .copied()
            .ok_or("the headset offers no blend mode")?;
        if let Ok(props) = instance.properties() {
            tracing::info!(
                "OpenXR runtime: {} {}",
                props.runtime_name,
                props.runtime_version
            );
        }
        Ok(Self {
            instance,
            system,
            blend_mode,
            session: None,
            exit: false,
        })
    }

    fn poll_events(&mut self) {
        let mut buffer = xr::EventDataBuffer::new();
        while let Ok(Some(event)) = self.instance.poll_event(&mut buffer) {
            match event {
                xr::Event::SessionStateChanged(change) => {
                    let Some(session) = &mut self.session else {
                        continue;
                    };
                    match change.state() {
                        xr::SessionState::READY => {
                            session.running = session.session.begin(VIEW_TYPE).is_ok();
                        }
                        xr::SessionState::STOPPING => {
                            let _ = session.session.end();
                            session.running = false;
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                            self.exit = true;
                        }
                        _ => {}
                    }
                }
                xr::Event::InstanceLossPending(_) => self.exit = true,
                _ => {}
            }
        }
    }
}

/// The runtime's space-separated extension list.
fn extension_names(list: xr::Result<String>) -> Result<Vec<CString>, String> {
    let list = list.map_err(|e| e.to_string())?;
    list.split_ascii_whitespace()
        .map(|name| CString::new(name).map_err(|e| e.to_string()))
        .collect()
}

fn eye_view(view: &xr::View) -> EyeView {
    let o = view.pose.orientation;
    let p = view.pose.position;
    EyeView {
        orientation: Quat::from_xyzw(o.x, o.y, o.z, o.w),
        position: Vec3::new(p.x, p.y, p.z),
        fov: Fov {
            left: view.fov.angle_left,
            right: view.fov.angle_right,
            up: view.fov.angle_up,
            down: view.fov.angle_down,
        },
    }
}

/// A `VK_MAKE_API_VERSION` value as OpenXR's version.
fn xr_version(vk: u32) -> xr::Version {
    xr::Version::new(
        (vk >> 22 & 0x7f) as u16,
        (vk >> 12 & 0x3ff) as u16,
        vk & 0xfff,
    )
}

impl StereoBackend for OpenXrBackend {
    fn instance_extensions(&self) -> Result<Vec<CString>, String> {
        extension_names(self.instance.vulkan_legacy_instance_extensions(self.system))
    }

    fn physical_device(&self, instance: RawHandle) -> Result<RawHandle, String> {
        // SAFETY: `instance` is the client's live VkInstance, created with
        // the extensions above.
        unsafe { self.instance.vulkan_graphics_device(self.system, instance) }
            .map_err(|e| e.to_string())
    }

    fn device_extensions(&self) -> Result<Vec<CString>, String> {
        extension_names(self.instance.vulkan_legacy_device_extensions(self.system))
    }

    fn start(&mut self, device: StereoDevice, formats: &[i64]) -> Result<StereoImages, String> {
        let requirements = self
            .instance
            .graphics_requirements::<xr::Vulkan>(self.system)
            .map_err(|e| e.to_string())?;
        let version = xr_version(device.api_version);
        if version < requirements.min_api_version_supported {
            return Err(format!(
                "the runtime needs Vulkan {}",
                requirements.min_api_version_supported
            ));
        }
        // SAFETY: the handles are the client's live instance, device and
        // queue, made to the runtime's requirements on the GPU it named; the
        // client ends the session before destroying them.
        let (session, frame_waiter, frame_stream) = unsafe {
            self.instance.create_session::<xr::Vulkan>(
                self.system,
                &xr::vulkan::SessionCreateInfo {
                    instance: device.instance,
                    physical_device: device.physical_device,
                    device: device.device,
                    queue_family_index: device.queue_family,
                    queue_index: device.queue_index,
                },
            )
        }
        .map_err(|e| format!("OpenXR session: {e}"))?;

        let offered = session
            .enumerate_swapchain_formats()
            .map_err(|e| e.to_string())?;
        let format = formats
            .iter()
            .copied()
            .find(|&f| offered.iter().any(|&o| i64::from(o) == f))
            .ok_or("the headset offers no 8-bit sRGB format")?;

        let views = self
            .instance
            .enumerate_view_configuration_views(self.system, VIEW_TYPE)
            .map_err(|e| e.to_string())?;
        let view = views.first().ok_or("the headset has no views")?;
        let (width, height) = (
            view.recommended_image_rect_width,
            view.recommended_image_rect_height,
        );
        let swapchain = session
            .create_swapchain(&xr::SwapchainCreateInfo {
                create_flags: xr::SwapchainCreateFlags::EMPTY,
                // TRANSFER_SRC for the window's mirror of the left eye.
                usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT
                    | xr::SwapchainUsageFlags::TRANSFER_SRC,
                format: format as u32,
                sample_count: 1,
                width,
                height,
                face_count: 1,
                array_size: 2,
                mip_count: 1,
            })
            .map_err(|e| format!("OpenXR swapchain: {e}"))?;
        let images = swapchain.enumerate_images().map_err(|e| e.to_string())?;
        let space = session
            .create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)
            .map_err(|e| e.to_string())?;

        self.session = Some(Session {
            session,
            frame_waiter,
            frame_stream,
            swapchain,
            extent: xr::Extent2Di {
                width: width as i32,
                height: height as i32,
            },
            space,
            running: false,
            frame: None,
            last_eyes: None,
        });
        Ok(StereoImages {
            format,
            width,
            height,
            images,
        })
    }

    fn begin_frame(&mut self) -> Option<StereoFrame> {
        self.poll_events();
        let session = self.session.as_mut()?;
        if !session.running {
            std::thread::sleep(IDLE_POLL);
            return None;
        }
        let state = session.frame_waiter.wait().ok()?;
        session.frame_stream.begin().ok()?;
        session.frame = Some((state.predicted_display_time, None));
        if !state.should_render {
            return None;
        }
        let (flags, views) = session
            .session
            .locate_views(VIEW_TYPE, state.predicted_display_time, &session.space)
            .ok()?;
        let views: [xr::View; 2] = views.try_into().ok()?;
        let image_index = session.swapchain.acquire_image().ok()?;
        session.frame = Some((state.predicted_display_time, Some(views)));
        session.swapchain.wait_image(xr::Duration::INFINITE).ok()?;
        // Tracking lost: hold the last good pose rather than snap to origin.
        let eyes = views.map(|view| eye_view(&view));
        if flags.contains(xr::ViewStateFlags::ORIENTATION_VALID) {
            session.last_eyes = Some(eyes);
        }
        Some(StereoFrame {
            image_index: image_index as usize,
            eyes: session.last_eyes.unwrap_or(eyes),
        })
    }

    fn end_frame(&mut self, rendered: bool) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some((display_time, views)) = session.frame.take() else {
            return;
        };
        if views.is_some() {
            let _ = session.swapchain.release_image();
        }
        let result = match views.filter(|_| rendered) {
            Some(views) => {
                let rect = xr::Rect2Di {
                    offset: xr::Offset2Di { x: 0, y: 0 },
                    extent: session.extent,
                };
                let eye = |layer: u32, view: &xr::View| {
                    xr::CompositionLayerProjectionView::new()
                        .pose(view.pose)
                        .fov(view.fov)
                        .sub_image(
                            xr::SwapchainSubImage::new()
                                .swapchain(&session.swapchain)
                                .image_array_index(layer)
                                .image_rect(rect),
                        )
                };
                let eyes = [eye(0, &views[0]), eye(1, &views[1])];
                let layer = xr::CompositionLayerProjection::new()
                    .space(&session.space)
                    .views(&eyes);
                session
                    .frame_stream
                    .end(display_time, self.blend_mode, &[&layer])
            }
            None => session.frame_stream.end(display_time, self.blend_mode, &[]),
        };
        if let Err(e) = result {
            tracing::warn!("OpenXR frame: {e}");
        }
    }

    fn should_exit(&self) -> bool {
        self.exit
    }
}
