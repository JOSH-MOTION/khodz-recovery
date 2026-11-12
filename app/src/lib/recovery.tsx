import { createContext, useCallback, useContext, useRef, useState, type ReactNode } from "react";
import { api, errorMessage } from "./api";
import type { RecoveryOutcome, RecoveryProgress, SelectionPart } from "./types";

export interface ActiveRecovery {
  jobId: string;
  label: string;
  destination: string;
  startedAt: number;
  progress: RecoveryProgress | null;
}

interface RecoveryState {
  active: ActiveRecovery | null;
  lastOutcome: RecoveryOutcome | null;
  lastError: string | null;
  start: (scanId: number, selection: SelectionPart[], destination: string, allowSameDisk: boolean, label: string) => void;
  cancel: () => void;
}

const Ctx = createContext<RecoveryState | null>(null);

/** One recovery at a time, app-wide: started from Results, followed on the Recovery page. */
export function RecoveryProvider({ children, onStarted }: { children: ReactNode; onStarted?: () => void }) {
  const [active, setActive] = useState<ActiveRecovery | null>(null);
  const [lastOutcome, setLastOutcome] = useState<RecoveryOutcome | null>(null);
  const [lastError, setLastError] = useState<string | null>(null);
  const ref = useRef<ActiveRecovery | null>(null);
  ref.current = active;

  const start = useCallback(
    (scanId: number, selection: SelectionPart[], destination: string, allowSameDisk: boolean, label: string) => {
      if (ref.current) return;
      const jobId =
        typeof crypto !== "undefined" && "randomUUID" in crypto ? crypto.randomUUID() : `rec-${Date.now()}`;
      setLastError(null);
      setActive({ jobId, label, destination, startedAt: Date.now(), progress: null });
      onStarted?.();
      api
        .startRecovery(scanId, selection, destination, allowSameDisk, jobId, (progress) =>
          setActive((a) => (a && a.jobId === jobId ? { ...a, progress } : a)),
        )
        .then(setLastOutcome)
        .catch((e) => setLastError(errorMessage(e)))
        .finally(() => setActive((a) => (a && a.jobId === jobId ? null : a)));
    },
    [onStarted],
  );

  const cancel = useCallback(() => {
    if (ref.current) void api.cancelJob(ref.current.jobId);
  }, []);

  return <Ctx.Provider value={{ active, lastOutcome, lastError, start, cancel }}>{children}</Ctx.Provider>;
}

export function useRecovery(): RecoveryState {
  const v = useContext(Ctx);
  if (!v) throw new Error("useRecovery outside RecoveryProvider");
  return v;
}
