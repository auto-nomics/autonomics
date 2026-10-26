import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: {
    strictPort: true,
  },
  build: {
    target: "es2022",
    rollupOptions: {
      output: {
        inlineDynamicImports: true,
        entryFileNames: "assets/gui.js",
        chunkFileNames: "assets/gui.js",
        assetFileNames: "assets/gui.[ext]",
      },
    },
  },
});
