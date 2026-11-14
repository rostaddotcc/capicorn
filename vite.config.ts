import { defineConfig } from "vite";

// Tauri expects a fixed port and no clearScreen so cargo logs stay visible.
// Multi-page: main app + the floating quick-access widget.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2021",
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      input: {
        main: "index.html",
        widget: "widget.html",
      },
    },
  },
});