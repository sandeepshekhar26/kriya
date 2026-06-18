import { useEffect, useRef } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import type { MeshData } from "./replicadKernel";

interface GL {
  scene: THREE.Scene;
  camera: THREE.PerspectiveCamera;
  renderer: THREE.WebGLRenderer;
  controls: OrbitControls;
  mesh?: THREE.Mesh;
  raf: number;
}

export function Viewport({ mesh, status }: { mesh: MeshData | null; status: string }) {
  const mountRef = useRef<HTMLDivElement>(null);
  const glRef = useRef<GL | null>(null);

  // Set up the scene once.
  useEffect(() => {
    const mount = mountRef.current;
    if (!mount || glRef.current) return; // initialise exactly once — survives the dev double-invoke

    const scene = new THREE.Scene();
    scene.background = new THREE.Color(0x0b0e14);

    const w0 = mount.clientWidth || 800;
    const h0 = mount.clientHeight || 600;
    const camera = new THREE.PerspectiveCamera(45, w0 / h0, 0.5, 5000);
    camera.up.set(0, 0, 1);
    camera.position.set(110, -120, 90);

    const renderer = new THREE.WebGLRenderer({ antialias: true });
    renderer.setSize(w0, h0);
    renderer.setPixelRatio(window.devicePixelRatio);
    mount.appendChild(renderer.domElement);

    const controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;

    scene.add(new THREE.AmbientLight(0xffffff, 0.65));
    const key = new THREE.DirectionalLight(0xffffff, 0.85);
    key.position.set(60, -40, 120);
    scene.add(key);
    const fill = new THREE.DirectionalLight(0x88aaff, 0.35);
    fill.position.set(-80, 60, 40);
    scene.add(fill);

    const grid = new THREE.GridHelper(220, 22, 0x2a3340, 0x171d27);
    grid.rotation.x = Math.PI / 2;
    scene.add(grid);

    const gl: GL = { scene, camera, renderer, controls, raf: 0 };
    const animate = () => {
      controls.update();
      renderer.render(scene, camera);
      gl.raf = requestAnimationFrame(animate);
    };
    animate();

    // Size to the actual container — a window 'resize' listener misses the initial layout
    // (clientHeight is often 0 when the effect first runs), which leaves a 0-size canvas.
    const resize = () => {
      const w = mount.clientWidth;
      const h = mount.clientHeight;
      if (w === 0 || h === 0) return;
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      renderer.setSize(w, h);
    };
    const ro = new ResizeObserver(resize);
    ro.observe(mount);
    glRef.current = gl;
    // No teardown: disposing/recreating the WebGL context on React's dev double-invoke is exactly
    // what blanks the canvas. This is a single-view app, so one context lives for the page lifetime.
  }, []);

  // Swap the mesh whenever the geometry changes.
  useEffect(() => {
    const gl = glRef.current;
    if (!gl) return;

    if (gl.mesh) {
      gl.scene.remove(gl.mesh);
      gl.mesh.geometry.dispose();
      (gl.mesh.material as THREE.Material).dispose();
      gl.mesh = undefined;
    }
    if (!mesh) return;

    const geom = new THREE.BufferGeometry();
    geom.setAttribute("position", new THREE.BufferAttribute(mesh.positions, 3));
    geom.setIndex(new THREE.BufferAttribute(mesh.indices, 1));
    geom.computeVertexNormals();
    const material = new THREE.MeshStandardMaterial({
      color: 0x6ea8fe,
      metalness: 0.15,
      roughness: 0.55,
      side: THREE.DoubleSide,
    });
    gl.mesh = new THREE.Mesh(geom, material);
    gl.scene.add(gl.mesh);
  }, [mesh]);

  // The three.js canvas is appended imperatively to `.viewport-canvas`, which has NO React
  // children — otherwise React's reconciliation of sibling content (the status overlay) wipes it.
  return (
    <div className="viewport">
      <div className="viewport-canvas" ref={mountRef} />
      {status && <div className="viewport-status">{status}</div>}
    </div>
  );
}
