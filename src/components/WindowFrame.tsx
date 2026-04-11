import { getCurrentWindow } from "@tauri-apps/api/window";

// Invisible resize edges around the window. Must be placed as the first child of the root element.
export function WindowFrame() {
  const win = getCurrentWindow();

  type ResizeDirection = "North" | "South" | "East" | "West" | "NorthEast" | "NorthWest" | "SouthEast" | "SouthWest";
  const startResize = (direction: ResizeDirection) => async (e: MouseEvent) => {
    e.preventDefault();
    await (win as any).startResizing(direction);
  };

  return (
    <>
      <div class="resize-edge resize-n" onMouseDown={startResize("North")} />
      <div class="resize-edge resize-s" onMouseDown={startResize("South")} />
      <div class="resize-edge resize-e" onMouseDown={startResize("East")} />
      <div class="resize-edge resize-w" onMouseDown={startResize("West")} />
      <div class="resize-edge resize-ne" onMouseDown={startResize("NorthEast")} />
      <div class="resize-edge resize-nw" onMouseDown={startResize("NorthWest")} />
      <div class="resize-edge resize-se" onMouseDown={startResize("SouthEast")} />
      <div class="resize-edge resize-sw" onMouseDown={startResize("SouthWest")} />
    </>
  );
}
