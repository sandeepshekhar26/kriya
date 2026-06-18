import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  // The OpenCascade WASM module must not be pre-bundled by esbuild.
  optimizeDeps: { exclude: ["replicad-opencascadejs"] },
});
