import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: {
    strictPort: true,
  },
  build: {
    target: "es2022",
  },
});
