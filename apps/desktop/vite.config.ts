import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The config runs in Node; declare the bit of `process` we use instead of
// pulling in @types/node for the whole project.
declare const process: { env: Record<string, string | undefined> };

const host = process.env.TAURI_DEV_HOST;

// https://v2.tauri.app/start/frontend/vite/
export default defineConfig({
  plugins: [react()],
  // Keep Rust/Tauri CLI output visible.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  preview: {
    port: 1420,
    strictPort: true,
  },
  // Only expose VITE_* and Tauri's TAURI_ENV_* build variables to the client.
  // (Deliberately not the whole TAURI_ prefix: that would also match signing
  // secrets such as TAURI_SIGNING_PRIVATE_KEY.)
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    // Tauri uses Chromium (WebView2) on Windows and WebKit on macOS/Linux.
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG),
    outDir: "dist",
    emptyOutDir: true,
  },
});
