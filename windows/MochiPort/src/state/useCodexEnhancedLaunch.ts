import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, ManagementError } from "../api/client";
import type { CodexEnhancedOperation } from "../api/types";

interface Options {
  fixtureMode: boolean;
  onError: (message: string) => void;
  onFeedback: (message: string) => void;
  onReady: () => void | Promise<void>;
}

const wait = (milliseconds: number) => new Promise((resolve) => window.setTimeout(resolve, milliseconds));

function isTerminal(operation: CodexEnhancedOperation): boolean {
  return operation.phase === "ready" || operation.phase === "failed" || operation.phase === "cancelled";
}

function isFeatureUnavailable(error: unknown): boolean {
  return error instanceof ManagementError && error.status === 404;
}

function messageFor(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function requestId(): string {
  return typeof crypto.randomUUID === "function"
    ? crypto.randomUUID().toLowerCase()
    : `windows-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function localOperation(
  id: string,
  phase: CodexEnhancedOperation["phase"],
  message: string,
  canCancel: boolean,
  startedAtMs = Date.now(),
  error?: string,
  recovery?: string,
): CodexEnhancedOperation {
  return {
    requestId: id,
    phase,
    startedAtMs,
    updatedAtMs: Date.now(),
    canCancel,
    message,
    error,
    recovery,
  };
}

export function useCodexEnhancedLaunch({ fixtureMode, onError, onFeedback, onReady }: Options) {
  const [operation, setOperation] = useState<CodexEnhancedOperation>();
  const [checkingPreflight, setCheckingPreflight] = useState(false);
  const [waitingForAppExit, setWaitingForAppExit] = useState(false);
  const [usesLegacyFallback, setUsesLegacyFallback] = useState(false);
  const [launchError, setLaunchError] = useState<string>();
  const generation = useRef(0);
  const beginInFlight = useRef<number | undefined>(undefined);
  const cancelInFlight = useRef<{ requestId: string; generation: number } | undefined>(undefined);
  const recoveryInFlight = useRef<number | undefined>(undefined);
  const mounted = useRef(true);
  const operationRef = useRef<CodexEnhancedOperation | undefined>(undefined);
  const activeMonitor = useRef<{ requestId: string; generation: number } | undefined>(undefined);
  const callbacks = useRef({ onError, onFeedback, onReady });

  useEffect(() => {
    callbacks.current = { onError, onFeedback, onReady };
  }, [onError, onFeedback, onReady]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      generation.current += 1;
    };
  }, []);

  const present = useCallback((next: CodexEnhancedOperation) => {
    if (!mounted.current) return;
    operationRef.current = next;
    setOperation(next);
    setLaunchError(next.phase === "failed" ? next.error ?? next.message : undefined);
  }, []);

  const finish = useCallback(async (next: CodexEnhancedOperation) => {
    present(next);
    if (next.phase === "ready") {
      callbacks.current.onFeedback("增强模式已就绪");
      await callbacks.current.onReady();
    }
  }, [present]);

  const monitor = useCallback(async (id: string, expectedGeneration: number) => {
    const currentMonitor = activeMonitor.current;
    if (currentMonitor?.requestId === id && currentMonitor.generation === expectedGeneration) return;
    activeMonitor.current = { requestId: id, generation: expectedGeneration };
    let failures = 0;
    try {
      while (mounted.current && generation.current === expectedGeneration) {
        await wait(failures >= 3 ? 2_000 : 400);
        if (!mounted.current || generation.current !== expectedGeneration) return;
        try {
          const response = await api.codexEnhancedOperation();
          if (!mounted.current || generation.current !== expectedGeneration) return;
          const next = response.operation;
          if (!next || next.requestId !== id) throw new Error("增强启动状态与当前请求不一致");
          failures = 0;
          present(next);
          if (isTerminal(next)) {
            await finish(next);
            return;
          }
        } catch (error) {
          failures += 1;
          if (failures >= 3 && mounted.current && generation.current === expectedGeneration) {
            const message = `暂时无法读取增强启动进度：${messageFor(error)}`;
            setLaunchError(message);
            callbacks.current.onError(message);
          }
        }
      }
    } finally {
      if (activeMonitor.current?.requestId === id && activeMonitor.current.generation === expectedGeneration) {
        activeMonitor.current = undefined;
      }
    }
  }, [finish, present]);

  const startLegacy = useCallback((id: string, expectedGeneration: number): boolean => {
    if (!mounted.current || generation.current !== expectedGeneration) return false;
    const startedAtMs = Date.now();
    setUsesLegacyFallback(true);
    present(localOperation(
      id,
      "launching",
      "正在等待旧版后台服务完成增强启动",
      true,
      startedAtMs,
      undefined,
      "取消只能停止本机等待，无法中止旧版后台服务中的启动。",
    ));
    void (async () => {
      try {
        await api.launchCodexEnhancedLegacy();
        if (!mounted.current || generation.current !== expectedGeneration) return;
        await finish(localOperation(id, "ready", "增强模式已就绪", false, startedAtMs));
      } catch (error) {
        if (!mounted.current || generation.current !== expectedGeneration) return;
        const message = messageFor(error);
        present(localOperation(id, "failed", "增强启动失败", false, startedAtMs, message));
        callbacks.current.onError(message);
      }
    })();
    return true;
  }, [finish, present]);

  const startManaged = useCallback(async (expectedGeneration: number): Promise<boolean> => {
    if (!mounted.current || generation.current !== expectedGeneration) return false;
    const id = requestId();
    try {
      const response = await api.startCodexEnhancedOperation(id);
      if (!response.operation) throw new Error("后台服务没有返回增强启动状态");
      if (!mounted.current || generation.current !== expectedGeneration) return false;
      setUsesLegacyFallback(false);
      present(response.operation);
      if (isTerminal(response.operation)) await finish(response.operation);
      else void monitor(response.operation.requestId, expectedGeneration);
      return true;
    } catch (error) {
      if (isFeatureUnavailable(error)) return startLegacy(id, expectedGeneration);

      // The request may have been accepted even if its response was lost.
      try {
        const recovered = (await api.codexEnhancedOperation()).operation;
        if (
          recovered
          && recovered.requestId === id
          && !isTerminal(recovered)
          && mounted.current
          && generation.current === expectedGeneration
        ) {
          setUsesLegacyFallback(false);
          present(recovered);
          void monitor(recovered.requestId, expectedGeneration);
          return true;
        }
      } catch {
        // Report the original start failure below.
      }
      if (mounted.current && generation.current === expectedGeneration) {
        const message = messageFor(error);
        setLaunchError(message);
        callbacks.current.onError(message);
      }
      return false;
    }
  }, [finish, monitor, present, startLegacy]);

  const waitForExit = useCallback(async (expectedGeneration: number) => {
    setWaitingForAppExit(true);
    while (mounted.current && generation.current === expectedGeneration) {
      await wait(1_000);
      if (!mounted.current || generation.current !== expectedGeneration) return;
      try {
        const preflight = await api.codexEnhancedPreflight();
        if (!mounted.current || generation.current !== expectedGeneration) return;
        if (!preflight.status.running) {
          setWaitingForAppExit(false);
          await startManaged(expectedGeneration);
          return;
        }
      } catch (error) {
        if (!mounted.current || generation.current !== expectedGeneration) return;
        setWaitingForAppExit(false);
        const message = `无法确认 Codex 是否已退出：${messageFor(error)}`;
        setLaunchError(message);
        callbacks.current.onError(message);
        return;
      }
    }
  }, [startManaged]);

  const begin = useCallback(async (): Promise<boolean> => {
    if (beginInFlight.current !== undefined) return false;
    const expectedGeneration = generation.current + 1;
    generation.current = expectedGeneration;
    beginInFlight.current = expectedGeneration;
    activeMonitor.current = undefined;
    operationRef.current = undefined;
    setOperation(undefined);
    setLaunchError(undefined);
    setWaitingForAppExit(false);
    setUsesLegacyFallback(false);
    setCheckingPreflight(true);

    try {
      if (fixtureMode) {
        const id = requestId();
        const startedAtMs = Date.now();
        present(localOperation(id, "launching", "正在启动 Codex", true, startedAtMs));
        void (async () => {
          await wait(650);
          if (mounted.current && generation.current === expectedGeneration) {
            await finish(localOperation(id, "ready", "增强模式已就绪", false, startedAtMs));
          }
        })();
        return true;
      }

      const preflight = await api.codexEnhancedPreflight();
      if (!mounted.current || generation.current !== expectedGeneration) return false;
      if (preflight.status.running) {
        void waitForExit(expectedGeneration);
        return true;
      }
      return await startManaged(expectedGeneration);
    } catch (error) {
      if (isFeatureUnavailable(error)) return startLegacy(requestId(), expectedGeneration);
      if (mounted.current && generation.current === expectedGeneration) {
        const message = messageFor(error);
        setLaunchError(message);
        callbacks.current.onError(message);
      }
      return false;
    } finally {
      if (beginInFlight.current === expectedGeneration) {
        beginInFlight.current = undefined;
      }
      if (mounted.current && generation.current === expectedGeneration) {
        setCheckingPreflight(false);
      }
    }
  }, [finish, fixtureMode, present, startLegacy, startManaged, waitForExit]);

  const cancel = useCallback(async () => {
    if (checkingPreflight) {
      const cancelledGeneration = generation.current;
      generation.current += 1;
      if (beginInFlight.current === cancelledGeneration) {
        beginInFlight.current = undefined;
      }
      setCheckingPreflight(false);
      setLaunchError(undefined);
      callbacks.current.onFeedback("已取消增强启动");
      return;
    }
    if (waitingForAppExit) {
      generation.current += 1;
      setWaitingForAppExit(false);
      setLaunchError(undefined);
      callbacks.current.onFeedback("已取消增强启动");
      return;
    }
    if (!operation || isTerminal(operation) || !operation.canCancel) return;
    const expectedGeneration = generation.current;
    const expectedRequestId = operation.requestId;

    if (usesLegacyFallback || fixtureMode) {
      generation.current = expectedGeneration + 1;
      const next = localOperation(
        operation.requestId,
        "cancelled",
        usesLegacyFallback ? "已停止等待旧版后台服务" : "增强启动已取消",
        false,
        operation.startedAtMs,
        undefined,
        usesLegacyFallback ? "旧版后台服务不支持服务端取消；Codex 仍可能继续启动。" : undefined,
      );
      present(next);
      callbacks.current.onFeedback("已取消增强启动");
      return;
    }

    const activeCancellation = cancelInFlight.current;
    if (
      activeCancellation?.requestId === expectedRequestId
      && activeCancellation.generation === expectedGeneration
    ) return;
    cancelInFlight.current = { requestId: expectedRequestId, generation: expectedGeneration };

    try {
      const response = await api.cancelCodexEnhancedOperation(expectedRequestId);
      if (
        !mounted.current
        || generation.current !== expectedGeneration
        || operationRef.current?.requestId !== expectedRequestId
        || isTerminal(operationRef.current)
      ) return;
      if (!response.operation) throw new Error("后台服务没有返回取消状态");
      if (response.operation.requestId !== expectedRequestId) {
        throw new Error("后台服务返回了其他增强启动请求的取消状态");
      }
      present(response.operation);
      if (isTerminal(response.operation)) {
        generation.current = expectedGeneration + 1;
      } else {
        // Cancellation is asynchronous. A 202 response normally reports a
        // non-terminal "正在取消" state, so keep observing until the daemon
        // publishes cancelled/failed/ready.
        void monitor(response.operation.requestId, expectedGeneration);
      }
      callbacks.current.onFeedback("已请求取消增强启动");
    } catch (error) {
      if (
        !mounted.current
        || generation.current !== expectedGeneration
        || operationRef.current?.requestId !== expectedRequestId
        || isTerminal(operationRef.current)
      ) return;
      const message = `取消失败：${messageFor(error)}`;
      setLaunchError(message);
      callbacks.current.onError(message);
    } finally {
      if (
        cancelInFlight.current?.requestId === expectedRequestId
        && cancelInFlight.current.generation === expectedGeneration
      ) {
        cancelInFlight.current = undefined;
      }
    }
  }, [checkingPreflight, fixtureMode, monitor, operation, present, usesLegacyFallback, waitingForAppExit]);

  const recover = useCallback(async () => {
    if (
      fixtureMode
      || beginInFlight.current !== undefined
      || recoveryInFlight.current !== undefined
      || (operationRef.current && !isTerminal(operationRef.current))
    ) return;
    const expectedGeneration = generation.current;
    const expectedOperationId = operationRef.current?.requestId;
    recoveryInFlight.current = expectedGeneration;
    try {
      const recovered = (await api.codexEnhancedOperation()).operation;
      if (
        !recovered
        || !mounted.current
        || generation.current !== expectedGeneration
        || beginInFlight.current !== undefined
        || operationRef.current?.requestId !== expectedOperationId
      ) return;
      present(recovered);
      if (!isTerminal(recovered)) {
        const monitorGeneration = expectedGeneration + 1;
        generation.current = monitorGeneration;
        setUsesLegacyFallback(false);
        void monitor(recovered.requestId, monitorGeneration);
      }
    } catch (error) {
      if (!isFeatureUnavailable(error)) {
        // Recovery is opportunistic; the main Codex status remains usable.
      }
    } finally {
      if (recoveryInFlight.current === expectedGeneration) {
        recoveryInFlight.current = undefined;
      }
    }
  }, [fixtureMode, monitor, present]);

  const inProgress = checkingPreflight || waitingForAppExit || Boolean(operation && !isTerminal(operation));
  const canCancel = checkingPreflight || waitingForAppExit || Boolean(operation?.canCancel && !isTerminal(operation));

  return useMemo(() => ({
    operation,
    waitingForAppExit,
    usesLegacyFallback,
    launchError,
    inProgress,
    canCancel,
    begin,
    cancel,
    recover,
  }), [begin, canCancel, cancel, inProgress, launchError, operation, recover, usesLegacyFallback, waitingForAppExit]);
}
