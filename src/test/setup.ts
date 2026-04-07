import { vi } from "vitest";

// Mock the Tauri core API so stores can be tested without a running Tauri process.
// Tests override invoke per-case via vi.mocked(invoke).mockResolvedValue(...)
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));
