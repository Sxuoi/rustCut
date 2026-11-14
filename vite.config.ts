import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";

const host = process.env.TAURI_DEV_HOST;

// https://v2.tauri.app/start/frontend/vite/
export default defineConfig({
  plugins: [svelte(), tailwindcss()],
  resolve: {
    alias: {
      $lib: fileURLToPath(new URL("./src/lib", import.meta.url)),
    },
  },
  // Tauri expects a fixed port and must see Rust errors.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: {
      ignored: ["**/src-tauri/**", "**/crates/**", "**/target/**", "**/target-*/**", "**/.fixtures/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    // WebView2 (Chromium) and WKWebView (Safari 15+) baselines.
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari15",
    // Vite 8 minifies with Oxc by default (esbuild is no longer bundled).
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
