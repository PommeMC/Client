# pomme-vr

Pomme in a PC VR headset, seated, through OpenXR. It's the desktop client with
an OpenXR runtime drawing the world to the headset; menus and the HUD stay on
the window, which mirrors the left eye.

```
cargo run -p pomme-vr
```

Any OpenXR runtime with Vulkan works (SteamVR, Meta's, Monado). The loader is
built in, so nothing ships beside the exe. With no runtime it exits with
"Couldn't start VR".

## Controls (Vivecraft's seated defaults)

- The headset looks around.
- The mouse moves the crosshair, up to 15° either side of where you faced.
  Push past that and you turn.
- Walking follows the crosshair; keyboard and gamepad are as on desktop.

## Testing without a headset

Meta's XR Simulator (Windows) stands in for a headset:

1. Install the Meta XR Simulator and set it as the active OpenXR runtime.
2. `cargo run -p pomme-vr`; the simulator's window shows both eyes, and its
   mouse controls move the head.
