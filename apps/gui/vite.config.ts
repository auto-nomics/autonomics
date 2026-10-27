import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";
import fs from "node:fs";
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
  plugins: [react(), tailwindcss(), {
    name: "zotero-reader-notice",
    generateBundle(_options, bundle) {
      // Preserve attribution after the minifier has processed the generated SDT
      // module. Complete corresponding sources/notices ship in the runtime.
      for (const output of Object.values(bundle)) {
        if (output.type === "chunk" && Object.keys(output.modules).some(id => id.endsWith("zotero-sdt.generated.js"))) {
          output.code = "/*! Zotero Reader semantic components: AGPL-3.0. Copyright Corporation for Digital Scholarship. Corresponding sources, revisions and notices: tools/zotero-extractor/ (included in release runtimes). */\n" + output.code;
        }
      }
    },
  }],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
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
