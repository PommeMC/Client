import { useEffect, useRef } from "react";
import { FunctionAnimation, PlayerObject, SkinViewer, WalkingAnimation } from "skinview3d";

const VIEW_WIDTH = 200;
const VIEW_HEIGHT = 300;
const CAMERA_FOV = 32;
const CAMERA_ZOOM = 0.9;
const LOOK_DISTANCE_PX = 520;
const MAX_YAW_RADIANS = 1.05;
const MAX_PITCH_RADIANS = 0.45;
const BODY_SHARE = 0.4;
const HEAD_SHARE = 0.6;
const TURN_SMOOTHING = 0.08;
const RESTING_YAW_RADIANS = 0.75;
const RESTING_PITCH_RADIANS = -0.08;
const ARM_SWAY_RADIANS = 0.04;
const ARM_REST_RADIANS = 0.06;
const BREATH_SPEED = 1.6;
const WALK_IN_MS = 1100;
const WALK_SPEED = 1.4;

export type PointerPosition = { x: number; y: number };

interface SkinPreviewProps {
  skinUrl: string;
  pointer: PointerPosition | null;
}

type Look = { yaw: number; pitch: number };

const RESTING_LOOK: Look = { yaw: RESTING_YAW_RADIANS, pitch: RESTING_PITCH_RADIANS };

function clamp(value: number, limit: number): number {
  const clamped = Math.max(-limit, Math.min(limit, value));
  return clamped;
}

function getLook(pointer: PointerPosition | null, canvas: HTMLCanvasElement | null): Look {
  const hasPointer = pointer !== null;
  if (!hasPointer) {
    return RESTING_LOOK;
  }
  const hasCanvas = canvas !== null;
  if (!hasCanvas) {
    return RESTING_LOOK;
  }
  const bounds = canvas.getBoundingClientRect();
  const headX = bounds.left + bounds.width / 2;
  const headY = bounds.top + bounds.height * 0.25;
  const yaw = clamp(Math.atan2(pointer.x - headX, LOOK_DISTANCE_PX), MAX_YAW_RADIANS);
  const pitch = clamp(Math.atan2(pointer.y - headY, LOOK_DISTANCE_PX), MAX_PITCH_RADIANS);
  const look: Look = { yaw, pitch };
  return look;
}

function breathe(player: PlayerObject, progress: number) {
  const sway = Math.sin(progress * BREATH_SPEED) * ARM_SWAY_RADIANS;
  player.skin.leftArm.rotation.z = ARM_REST_RADIANS + sway;
  player.skin.rightArm.rotation.z = -ARM_REST_RADIANS - sway;
}

export default function SkinPreview({ skinUrl, pointer }: SkinPreviewProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const lookRef = useRef<Look>(RESTING_LOOK);

  useEffect(() => {
    lookRef.current = getLook(pointer, canvasRef.current);
  }, [pointer]);

  useEffect(() => {
    const canvas = canvasRef.current;
    const hasCanvas = canvas !== null;
    if (!hasCanvas) {
      return;
    }

    const viewer = new SkinViewer({
      canvas,
      width: VIEW_WIDTH,
      height: VIEW_HEIGHT,
      skin: skinUrl,
      pixelRatio: "match-device",
    });
    viewer.renderer.setClearColor(0x000000, 0);
    viewer.fov = CAMERA_FOV;
    viewer.zoom = CAMERA_ZOOM;
    viewer.autoRotate = false;
    viewer.controls.enableRotate = false;
    viewer.controls.enableZoom = false;
    viewer.controls.enablePan = false;

    const walk = new WalkingAnimation();
    walk.speed = WALK_SPEED;
    walk.headBobbing = false;
    viewer.animation = walk;
    const settle = setTimeout(() => {
      viewer.animation = new FunctionAnimation(breathe);
    }, WALK_IN_MS);

    let frame = 0;
    const follow = () => {
      const { yaw, pitch } = lookRef.current;
      const body = viewer.playerObject;
      const head = viewer.playerObject.skin.head;
      body.rotation.y += (yaw * BODY_SHARE - body.rotation.y) * TURN_SMOOTHING;
      head.rotation.y += (yaw * HEAD_SHARE - head.rotation.y) * TURN_SMOOTHING;
      head.rotation.x += (pitch - head.rotation.x) * TURN_SMOOTHING;
      frame = requestAnimationFrame(follow);
    };
    frame = requestAnimationFrame(follow);

    return () => {
      clearTimeout(settle);
      cancelAnimationFrame(frame);
      viewer.dispose();
    };
  }, [skinUrl]);

  return (
    <canvas
      ref={canvasRef}
      className="pointer-events-none animate-walk-in select-none"
      style={{ width: VIEW_WIDTH, height: VIEW_HEIGHT }}
    />
  );
}
