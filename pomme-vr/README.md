# pomme-vr

Pomme in a PC VR headset, seated, through OpenXR. It's the desktop client with
an OpenXR runtime drawing the world to the headset, and the HUD, screens and
menus on a panel floating in it. The window mirrors the left eye in the world,
and the panel in menus.

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
- The HUD rides along with the crosshair. A screen (inventory, pause, chat)
  opens a metre ahead and stays put; the title screens trail your heading.
  The window's mouse points at the panel.
- Options → VR Settings: Reset Origin recentres; HUD Distance and HUD Size
  move the HUD panel.

## Testing without a headset

Meta's XR Simulator (Windows) stands in for a headset:

1. Install the Meta XR Simulator and set it as the active OpenXR runtime.
2. `cargo run -p pomme-vr`; the simulator's window shows both eyes, and its
   mouse controls move the head.
