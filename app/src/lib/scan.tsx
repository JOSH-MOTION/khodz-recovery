import { createContext, useCallback, useContext, useRef, useState, type ReactNode } from "react";
import { api, errorMessage } from "./api";
import type { ScanOutcome, ScanProgress, ScanSource } from "./types";

export type ScanModeChoice = "quick" | "deep";

export interface ActiveScan {
  jobId: string;
  label: string;
  mode: ScanModeChoice;
  startedAt: number;
  progress: ScanProgress | null;
}

interface ScanState {
  active: ActiveScan | null;
  lastOutcome: ScanOutcome | null;
  lastError: string | null;
  start: (source: ScanSource, label: string, mode: ScanModeChoice) => void;
  cancel: () => void;
}

const Ctx = createContext<ScanState | null>(null);

function newJobId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `scan-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

/** One scan at a time, app-wide: started from Drives / Disk Images, watched on Results. */
export function ScanProvider({ children, onStarted }: { children: ReactNode; onStarted?: () => void }) {
  const [active, setActive] = useState<ActiveScan | null>(null);
  const [lastOutcome, setLastOutcome] = useState<ScanOutcome | null>(null);
  const [lastError, setLastError] = useState<string | null>(null);
  const activeRef = useRef<ActiveScan | null>(null);
  activeRef.current = active;

  const start = useCallback(
    (source: ScanSource, label: string, mode: ScanModeChoice) => {
      if (activeRef.current) return;
      const jobId = newJobId();
      setLastError(null);
      setActive({ jobId, label, mode, startedAt: Date.now(), progress: null });
      onStarted?.();
      api
        .startScan(source, mode, jobId, (progress) =>
          setActive((a) => (a && a.jobId === jobId ? { ...a, progress } : a)),
        )
        .then((outcome) => setLastOutcome(outcome))
        .catch((e) => setLastError(errorMessage(e)))
        .finally(() => setActive((a) => (a && a.jobId === jobId ? null : a)));
    },
    [onStarted],
  );

  const cancel = useCallback(() => {
    const a = activeRef.current;
    if (a) void api.cancelJob(a.jobId);
  }, []);

  return <Ctx.Provider value={{ active, lastOutcome, lastError, start, cancel }}>{children}</Ctx.Provider>;
}

export function useScan(): ScanState {
  const v = useContext(Ctx);
  if (!v) throw new Error("useScan outside ScanProvider");
  return v;
}
