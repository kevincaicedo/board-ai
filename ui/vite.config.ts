import { defineConfig } from "vite";

// Tauri serves the dev build from a fixed port and embeds the production build.
// WKWebView on macOS 14 matches Safari 17, the oldest engine BoardAI supports.
export default defineConfig({
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: "safari17", outDir: "dist" },
});
