import { useEffect, useRef } from "react";
import { SkinViewer, WalkingAnimation } from "skinview3d";

interface SkinRunnerProps {
  skinUrl: string | null;
  progress: number;
}

export default function SkinRunner({ skinUrl, progress }: SkinRunnerProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const viewerRef = useRef<SkinViewer | null>(null);

  useEffect(() => {
    if (!canvasRef.current) return;

    const viewer = new SkinViewer({
      canvas: canvasRef.current,
      width: 32,
      height: 32,
      skin: skinUrl || undefined,
    });
    viewer.renderer.setClearColor(0x000000, 0);

    viewer.camera.rotation.x = 0;
    viewer.camera.rotation.y = -Math.PI / 2;
    viewer.camera.position.set(-30, 2, 0);
    viewer.fov = 30;
    viewer.animation = new WalkingAnimation();
    viewer.animation.speed = 1.5;
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
      className="skin-runner"
      style={{ left: `calc(${progress * 100}% - 16px)` }}
    />
  );
}
