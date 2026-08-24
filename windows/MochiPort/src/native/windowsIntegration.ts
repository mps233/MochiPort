import { invoke } from "@tauri-apps/api/core";
import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";

const AUTOSTART_PREVIEW_KEY = "mochiport.autostart-preview";

export function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window || "__TAURI__" in window;
}

export async function autostartEnabled(): Promise<boolean> {
  if (!isTauriRuntime()) return localStorage.getItem(AUTOSTART_PREVIEW_KEY) === "on";
  return isEnabled();
}

export async function setAutostartEnabled(enabled: boolean): Promise<boolean> {
  if (!isTauriRuntime()) {
    localStorage.setItem(AUTOSTART_PREVIEW_KEY, enabled ? "on" : "off");
    return enabled;
  }
  if (enabled) await enable();
  else await disable();
  return isEnabled();
}

export async function ensureNotificationPermission(): Promise<boolean> {
  if (!isTauriRuntime()) return true;
  if (await isPermissionGranted()) return true;
  return await requestPermission() === "granted";
}

export async function showNativeNotification(title: string, body: string, withSound = false): Promise<boolean> {
  if (!await ensureNotificationPermission()) return false;
  if (!isTauriRuntime()) return true;
  sendNotification({ title, body, ...(withSound ? { sound: "Default" } : {}) });
  return true;
}

export async function openNativeLogDirectory(): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("open_log_directory");
}

export async function openNativeReleasePage(url: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("open_release_page", { url });
}
