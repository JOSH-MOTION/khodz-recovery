import { useCallback, useEffect, useState } from "react";
import { errorMessage } from "./api";

export type AsyncState<T> =
  | { status: "loading" }
  | { status: "error"; error: string }
  | { status: "ready"; data: T };

/** Runs `load` on mount (and on `reload()`), discarding results from stale runs. */
export function useAsync<T>(load: () => Promise<T>) {
  const [state, setState] = useState<AsyncState<T>>({ status: "loading" });
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    let active = true;
    setState({ status: "loading" });
    load()
      .then((data) => active && setState({ status: "ready", data }))
      .catch((e) => active && setState({ status: "error", error: errorMessage(e) }));
    return () => {
      active = false;
    };
    // `load` is intentionally excluded: callers pass inline closures; reloads go through `nonce`.
  }, [nonce]);

  const reload = useCallback(() => setNonce((n) => n + 1), []);
  return { state, reload };
}
