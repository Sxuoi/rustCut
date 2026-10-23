import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Vite config for the RustCut UI. `npm run build` output (ui/dist) is what
// the Tauri shell packages as frontendDist.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "es2022",
    outDir: "dist",
  },
});
