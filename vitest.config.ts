import { defineConfig } from "vitest/config";
import solid from "vite-plugin-solid";

export default defineConfig({
  plugins: [solid()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["src/test/setup.ts"],
    // Exclude Tauri CLI and src-tauri
    exclude: ["src-tauri/**", "node_modules/**"],
  },
});
