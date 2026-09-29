import { WebviewWindow } from "@tauri-apps/api/webviewWindow";

interface ChildWindowConfig {
  /**
   * The webview's label. Keep it to 1–64 of `[A-Za-z0-9_-]`: Core uses it
   * as the window's write origin (phase 5d), and drops one that isn't.
   * Today's labels: `main`, `theme-editor`, `log-viewer`.
   */
  label: string;
  url: string;
  title: string;
  width: number;
  height: number;
  minWidth: number;
  minHeight: number;
}

const windowRefs = new Map<string, WebviewWindow>();

/**
 * Opens a child window with the given config.
 * If the window is already open, focuses it instead.
 */
export async function openChildWindow(config: ChildWindowConfig): Promise<void> {
  try {
    const existing = await WebviewWindow.getByLabel(config.label);
    if (existing) {
      await existing.setFocus();
      return;
    }

    const window = new WebviewWindow(config.label, {
      url: config.url,
      title: config.title,
      width: config.width,
      height: config.height,
      minWidth: config.minWidth,
      minHeight: config.minHeight,
      center: true,
      resizable: true,
      decorations: true,
      alwaysOnTop: false,
    });

    windowRefs.set(config.label, window);

    void window.once("tauri://error", (e) => {
      console.error(`Failed to create ${config.label} window:`, e);
      windowRefs.delete(config.label);
    });

    void window.once("tauri://destroyed", () => {
      windowRefs.delete(config.label);
    });
  } catch (error) {
    console.error(`Error opening ${config.label} window:`, error);
  }
}

/**
 * Closes a child window by label if it's open.
 */
export async function closeChildWindow(label: string): Promise<void> {
  const window = windowRefs.get(label);
  if (window) {
    await window.close();
    windowRefs.delete(label);
  }
}

/**
 * Checks if a child window is currently open.
 */
export function isChildWindowOpen(label: string): boolean {
  return windowRefs.has(label);
}
