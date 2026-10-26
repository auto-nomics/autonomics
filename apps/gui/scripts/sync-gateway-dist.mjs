import { cp, mkdir, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const guiDir = path.dirname(fileURLToPath(import.meta.url));
const source = path.join(guiDir, "..", "dist");
const destination = path.join(
  guiDir,
  "..",
  "..",
  "..",
  "crates",
  "gateway",
  "gui-dist",
);

await rm(destination, { recursive: true, force: true });
await mkdir(destination, { recursive: true });
await cp(source, destination, { recursive: true });

console.log(`Synced GUI assets to ${path.relative(process.cwd(), destination)}`);
