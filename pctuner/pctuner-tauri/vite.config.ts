import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The same frontend runs inside Tauri and in a normal browser (with mocked command data).
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: "es2022", outDir: "dist", emptyOutDir: true },
});
