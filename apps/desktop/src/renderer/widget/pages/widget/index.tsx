import { useEffect } from "react";
import { FloatingButton } from "./components/FloatingButton";
import { useWidgetNotifications } from "../../hooks/useWidgetNotifications";
import { api } from "@/trpc/react";

export function WidgetPage() {
  useWidgetNotifications();

  const utils = api.useUtils();

  // Listen for settings-changed events from main process to sync mode state
  useEffect(() => {
    const handleSettingsChanged = () => {
      void utils.settings.getModes.invalidate();
      void utils.settings.getActiveMode.invalidate();
    };

    window.electronAPI.on("settings-changed", handleSettingsChanged);
    return () => {
      window.electronAPI.off("settings-changed", handleSettingsChanged);
    };
  }, [utils]);

  return <FloatingButton />;
}
