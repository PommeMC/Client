import { useEffect, useRef } from "react";
import { SkinViewer, WalkingAnimation } from "skinview3d";

const CANVAS_SIZE = 32;
const CAMERA_DISTANCE = -30;
const CAMERA_HEIGHT = 2;
const CAMERA_FOV = 30;
const WALK_SPEED = 1.5;
const HALF_CANVAS_PX = 16;

interface SkinRunnerProps {
  skinUrl: string | null;
  progress: number;
}

export default function SkinRunner({ skinUrl, progress }: SkinRunnerProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const viewerRef = useRef<SkinViewer | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const hasCanvas = canvas !== null;
    if (!hasCanvas) {
      return;
    }

    const viewer = new SkinViewer({
      canvas,
      width: CANVAS_SIZE,
      height: CANVAS_SIZE,
      skin: skinUrl || undefined,
    });
    viewer.renderer.setClearColor(0x000000, 0);

    viewer.camera.rotation.x = 0;
    viewer.camera.rotation.y = -Math.PI / 2;
    viewer.camera.position.set(CAMERA_DISTANCE, CAMERA_HEIGHT, 0);
    viewer.fov = CAMERA_FOV;
    viewer.animation = new WalkingAnimation();
    viewer.animation.speed = WALK_SPEED;
    viewer.autoRotate = false;
    viewerRef.current = viewer;

    return () => {
      viewer.dispose();
      viewerRef.current = null;
    };
  }, [skinUrl]);

  return (
    <canvas
      ref={canvasRef}
      className="pointer-events-none absolute top-1/2 z-10 size-8 -translate-y-1/2 transition-[left] duration-300"
      style={{ left: `calc(${progress * 100}% - ${HALF_CANVAS_PX}px)` }}
    />
  );
}
