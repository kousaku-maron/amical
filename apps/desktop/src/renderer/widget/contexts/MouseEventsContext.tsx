import React, { createContext, useCallback, useContext, useMemo, useRef } from "react";
import { api } from "@/trpc/react";

interface MouseEventsControl {
  /** Increment refCount. When 0→1, sends setIgnoreMouseEvents(false). */
  acquire: () => void;
  /** Decrement refCount. When 1→0, sends setIgnoreMouseEvents(true). */
  release: () => void;
}

const MouseEventsContext = createContext<MouseEventsControl | null>(null);

/**
 * Provides centralized setIgnoreMouseEvents management via refCount.
 * Multiple consumers (FloatingButton, toast hover) can independently
 * acquire/release without conflicting IPC calls.
 */
export const MouseEventsProvider: React.FC<{ children: React.ReactNode }> = ({
  children,
}) => {
  const refCountRef = useRef(0);
  const setIgnoreMouseEvents = api.widget.setIgnoreMouseEvents.useMutation();
  // Keep a stable ref to mutate so acquire/release callbacks never go stale
  const mutateRef = useRef(setIgnoreMouseEvents.mutate);
  mutateRef.current = setIgnoreMouseEvents.mutate;

  const acquire = useCallback(() => {
    refCountRef.current++;
    if (refCountRef.current === 1) {
      mutateRef.current({ ignore: false });
    }
  }, []);

  const release = useCallback(() => {
    if (refCountRef.current <= 0) return;
    refCountRef.current--;
    if (refCountRef.current === 0) {
      mutateRef.current({ ignore: true });
    }
  }, []);

  const value = useMemo(() => ({ acquire, release }), [acquire, release]);

  return (
    <MouseEventsContext.Provider value={value}>
      {children}
    </MouseEventsContext.Provider>
  );
};

export const useMouseEvents = () => {
  const ctx = useContext(MouseEventsContext);
  if (!ctx) {
    throw new Error("useMouseEvents must be used within MouseEventsProvider");
  }
  return ctx;
};
