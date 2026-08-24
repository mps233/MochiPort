import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, X } from "lucide-react";
import { useEffect, useState } from "react";

function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window || "__TAURI__" in window;
}

export function TitleBar() {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    if (!isTauri()) return;
    const appWindow = getCurrentWindow();
    void appWindow.isMaximized().then(setMaximized);
    let dispose: (() => void) | undefined;
    void appWindow.onResized(() => void appWindow.isMaximized().then(setMaximized)).then((value) => { dispose = value; });
    return () => dispose?.();
  }, []);

  const action = async (kind: "minimize" | "maximize" | "close") => {
    if (!isTauri()) return;
    const appWindow = getCurrentWindow();
    if (kind === "minimize") await appWindow.minimize();
    if (kind === "maximize") await appWindow.toggleMaximize();
    if (kind === "close") await appWindow.close();
  };

  return (
    <header className="titlebar">
      <div
        className="titlebar__drag"
        data-tauri-drag-region
        onDoubleClick={() => void action("maximize")}
      >
        <div className="titlebar__brand" data-tauri-drag-region>
          <span className="brand-mark brand-mark--small" aria-hidden><span /></span>
          <span data-tauri-drag-region>MochiPort</span>
        </div>
      </div>
      <div className="window-controls">
        <button type="button" aria-label="最小化" onClick={() => void action("minimize")}><Minus size={14} /></button>
        <button type="button" aria-label={maximized ? "还原" : "最大化"} onClick={() => void action("maximize")}>
          <Square size={maximized ? 12 : 13} />
        </button>
        <button type="button" className="window-controls__close" aria-label="关闭" onClick={() => void action("close")}><X size={15} /></button>
      </div>
    </header>
  );
}
