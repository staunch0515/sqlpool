import { useEffect, useState } from "react";
import { invoke as tauriInvoke } from "@tauri-apps/api/core";

// Every backend call goes through here, so the app knows when something is still working.
let pending = 0;
const listeners = new Set<(n: number) => void>();

function track(delta: number) {
  pending += delta;
  listeners.forEach((l) => l(pending));
}

/** Same as Tauri's invoke, but counts the calls that are still running. */
export function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  track(1);
  return tauriInvoke<T>(cmd, args).finally(() => track(-1));
}

/** How many backend calls are running right now. */
export function usePending(): number {
  const [n, setN] = useState(pending);
  useEffect(() => {
    listeners.add(setN);
    return () => {
      listeners.delete(setN);
    };
  }, []);
  return n;
}

const MIN_SHOW_MS = 450;

/**
 * Whether anything is working, held on for a moment after it finishes. A call that takes 20 ms would
 * otherwise flash by unseen; this keeps the progress line and the spinners up long enough to notice.
 */
export function useBusy(): boolean {
  const n = usePending();
  const [busy, setBusy] = useState(false);
  const [since, setSince] = useState(0);
  useEffect(() => {
    if (n > 0) {
      setBusy(true);
      setSince((s) => s || Date.now());
      return;
    }
    const wait = Math.max(0, MIN_SHOW_MS - (Date.now() - since));
    const t = setTimeout(() => {
      setBusy(false);
      setSince(0);
    }, wait);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [n]);
  return busy;
}
