import react from "@vitejs/plugin-react";
import crossOriginIsolation from "vite-plugin-cross-origin-isolation";
import { defineConfig } from "vite";

const serverUrl = process.env.AQUASCOPE_SERVER ?? "http://127.0.0.1:8008";
export default defineConfig(({ mode }) => ({
  build: {
    target: "es2022"
  },
  esbuild: {
    target: "es2022"
  },
  base: "./",
  define: {
    "process.env.NODE_ENV": JSON.stringify(mode)
  },
  plugins: [react(), crossOriginIsolation()]
}));
