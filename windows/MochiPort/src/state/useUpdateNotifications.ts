import { invoke } from "@tauri-apps/api/core";
import {
  createContext,
  createElement,
  type PropsWithChildren,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { isTauriRuntime, showNativeNotification } from "../native/windowsIntegration";
import {
  loadNotificationMessageStyle,
  NOTIFICATION_SOUND_STORAGE_KEY,
  NOTIFY_UPDATE_STORAGE_KEY,
  resolveNotificationTitle,
} from "../utils/notificationMessages";

export interface UpdateCheckResult {
  currentVersion: string;
  latestVersion: string;
  updateAvailable: boolean;
  releaseUrl: string;
}

export type UpdateCheckStatus = "idle" | "checking" | "success" | "error" | "unsupported";

export interface UpdateState {
  status: UpdateCheckStatus;
  result?: UpdateCheckResult;
  error?: string;
  checkedAt?: number;
  dismissed: boolean;
  checkNow: () => Promise<UpdateCheckResult | undefined>;
  dismiss: () => void;
}

export const STARTUP_UPDATE_CHECK_DELAY_MS = 15_000;

function isSafeReleaseUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return url.protocol === "https:"
      && url.hostname.toLowerCase() === "github.com"
      && !url.username
      && !url.password
      && !url.port
      && !url.search
      && !url.hash
      && url.pathname.toLowerCase().startsWith("/mps233/mochiport/releases/");
  } catch {
    return false;
  }
}

function isUpdateCheckResult(value: unknown): value is UpdateCheckResult {
  if (typeof value !== "object" || value === null) return false;
  const result = value as Partial<UpdateCheckResult>;
  return typeof result.currentVersion === "string"
    && typeof result.latestVersion === "string"
    && typeof result.updateAvailable === "boolean"
    && typeof result.releaseUrl === "string"
    && isSafeReleaseUrl(result.releaseUrl);
}

export async function checkForAppUpdates(): Promise<UpdateCheckResult> {
  if (!isTauriRuntime()) throw new Error("当前运行环境不支持更新检查");
  const result = await invoke<unknown>("check_for_updates");
  if (!isUpdateCheckResult(result)) throw new Error("更新检查响应格式无效");
  return result;
}

function displayVersion(version: string): string {
  const normalized = version.trim().replace(/^[vV]+/u, "");
  return `v${normalized}`;
}

const UpdateContext = createContext<UpdateState | null>(null);

function updateErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function UpdateProvider({ children }: PropsWithChildren) {
  const [status, setStatus] = useState<UpdateCheckStatus>("idle");
  const [result, setResult] = useState<UpdateCheckResult>();
  const [error, setError] = useState<string>();
  const [checkedAt, setCheckedAt] = useState<number>();
  const [dismissed, setDismissed] = useState(false);
  const previousAvailableVersion = useRef<string | undefined>(undefined);
  const requestRef = useRef<Promise<UpdateCheckResult | undefined> | null>(null);

  const checkNow = useCallback(async (): Promise<UpdateCheckResult | undefined> => {
    if (!isTauriRuntime()) {
      setStatus("unsupported");
      setError(undefined);
      return undefined;
    }

    if (requestRef.current) return requestRef.current;

    setStatus("checking");
    setError(undefined);
    const request = checkForAppUpdates()
      .then((nextResult) => {
        setResult(nextResult);
        setStatus("success");
        setCheckedAt(Date.now());
        if (!nextResult.updateAvailable || previousAvailableVersion.current !== nextResult.latestVersion) {
          setDismissed(false);
        }
        previousAvailableVersion.current = nextResult.updateAvailable ? nextResult.latestVersion : undefined;
        return nextResult;
      })
      .catch((requestError: unknown) => {
        setStatus("error");
        setError(updateErrorMessage(requestError));
        setCheckedAt(Date.now());
        return undefined;
      })
      .finally(() => {
        requestRef.current = null;
      });
    requestRef.current = request;
    return request;
  }, []);

  const dismiss = useCallback(() => {
    setDismissed(true);
  }, []);

  const value = useMemo<UpdateState>(() => ({
    status,
    result,
    error,
    checkedAt,
    dismissed,
    checkNow,
    dismiss,
  }), [checkedAt, checkNow, dismiss, dismissed, error, result, status]);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let active = true;
    const timer = window.setTimeout(() => {
      void checkNow().then(async (nextResult) => {
        if (!active || !nextResult?.updateAvailable || localStorage.getItem(NOTIFY_UPDATE_STORAGE_KEY) === "off") return;
        const title = resolveNotificationTitle(
          "update",
          "MochiPort 有新版本",
          loadNotificationMessageStyle(localStorage),
        );
        await showNativeNotification(
          title,
          displayVersion(nextResult.latestVersion),
          localStorage.getItem(NOTIFICATION_SOUND_STORAGE_KEY) === "on",
        );
      }).catch(() => undefined);
    }, STARTUP_UPDATE_CHECK_DELAY_MS);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [checkNow]);

  return createElement(UpdateContext.Provider, { value }, children);
}

export function useUpdateState(): UpdateState {
  const value = useContext(UpdateContext);
  if (!value) throw new Error("useUpdateState must be used within UpdateProvider");
  return value;
}
