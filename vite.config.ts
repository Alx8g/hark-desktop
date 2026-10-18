import { defineConfig } from "vite";
import { resolve } from "node:path";

// Local shell pages only (loading screen, settings, quick ask, plan island).
// The main Hark experience is the real https://hark.com loaded in WebView2.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: "es2021",
    minify: "esbuild",
    sourcemap: false,
    rollupOptions: {
      input: {
        splash: resolve(__dirname, "index.html"),
        settings: resolve(__dirname, "settings.html"),
        quick: resolve(__dirname, "quick.html"),
        island: resolve(__dirname, "island.html"),
      },
    },
  },
});
