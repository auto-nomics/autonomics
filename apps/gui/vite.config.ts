import { defineConfig } from "vite";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";

const gatewayTarget = `http://${
  process.env.AUTONOMICS_HTTP_API_ADDR ?? "127.0.0.1:8765"
}`;

function devGatewayToken(): string {
  if (process.env.AUTONOMICS_HTTP_API_TOKEN) {
    return process.env.AUTONOMICS_HTTP_API_TOKEN;
  }
  const stateDir =
    process.env.AUTONOMICS_STATE_DIR ?? path.join(os.homedir(), ".autonomics");
  try {
    return fs
      .readFileSync(path.join(stateDir, "gateway.token"), "utf8")
      .trim();
  } catch {
    return "";
  }
}

const gatewayToken = devGatewayToken();

export default defineConfig({
  clearScreen: false,
  server: {
    strictPort: true,
    proxy: {
      "/api": {
        target: gatewayTarget,
        changeOrigin: true,
        // The browser dev page does not have the daemon-written token file.
        // Keep the secret in Node and attach it only to proxied API requests.
        configure: (proxy) => {
          proxy.on("proxyReq", (proxyReq) => {
            if (gatewayToken) {
              proxyReq.setHeader("Authorization", `Bearer ${gatewayToken}`);
            }
          });
        },
      },
    },
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
