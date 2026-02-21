import React, { useState, useEffect, useRef } from "react";
import { Check } from "lucide-react";

interface ModeInfo {
  id: string;
  name: string;
}

interface ModeCycledPayload {
  activeModeId: string;
  modes: ModeInfo[];
}

const AUTO_DISMISS_MS = 1000;

export const ModeSwitchOverlay: React.FC = () => {
  const [data, setData] = useState<ModeCycledPayload | null>(null);
  const [visible, setVisible] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const handleModeCycled = (payload: ModeCycledPayload) => {
      setData(payload);
      setVisible(true);

      // Reset auto-dismiss timer
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => {
        setVisible(false);
      }, AUTO_DISMISS_MS);
    };

    window.electronAPI.on("mode-cycled", handleModeCycled);
    return () => {
      window.electronAPI.off("mode-cycled", handleModeCycled);
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);

  // Clear data after fade-out transition completes
  useEffect(() => {
    if (!visible && data) {
      const cleanup = setTimeout(() => setData(null), 200);
      return () => clearTimeout(cleanup);
    }
  }, [visible, data]);

  if (!data) return null;

  return (
    <div
      className="fixed inset-0 flex items-center justify-center pointer-events-none z-50"
      style={{ pointerEvents: "none" }}
    >
      <div
        className={`
          bg-black/80 backdrop-blur-xl rounded-2xl
          ring-[1px] ring-white/15
          shadow-[0px_0px_30px_0px_rgba(0,0,0,0.50)]
          px-2 py-2 min-w-[200px]
          transition-all duration-150
          ${visible ? "opacity-100 scale-100" : "opacity-0 scale-95"}
        `}
      >
        {data.modes.map((mode) => {
          const isActive = mode.id === data.activeModeId;
          return (
            <div
              key={mode.id}
              className={`
                flex items-center gap-3 px-4 py-2.5 rounded-xl
                transition-colors duration-100
                ${isActive ? "bg-white/15" : ""}
              `}
            >
              <span
                className={`text-sm ${isActive ? "text-white font-medium" : "text-white/50"}`}
              >
                {mode.name}
              </span>
              {isActive && (
                <Check className="w-3.5 h-3.5 text-white/70 ml-auto" />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
};
