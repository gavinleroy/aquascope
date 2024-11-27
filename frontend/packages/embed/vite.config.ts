import fs from "node:fs";
import { resolve } from "node:path";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const manifest = JSON.parse(fs.readFileSync("package.json", "utf-8"));
export default defineConfig(({ mode }) => ({
  build: {
    target: "es2022",
    lib: {
      entry: resolve(__dirname, "src/main.tsx"),
      name: "AquascopeEmbed",
      formats: ["iife"]
    },
    rollupOptions: {
      external: Object.keys(manifest.dependencies || {})
    }
  },
  esbuild: {
    target: "es2022"
  },
  define: {
    "process.env.NODE_ENV": JSON.stringify(mode)
  },
  plugins: [react()]
}));
