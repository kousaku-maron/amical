import { useEffect } from "react";
import { FloatingButton } from "./components/FloatingButton";
import { ModeSwitchOverlay } from "../../components/ModeSwitchOverlay";
import { useWidgetNotifications } from "../../hooks/useWidgetNotifications";
import { MouseEventsProvider } from "../../contexts/MouseEventsContext";
import { api } from "@/trpc/react";

export function WidgetPage() {
  return (
    <MouseEventsProvider>
      <WidgetPageContent />
    </MouseEventsProvider>
  );
}

function WidgetPageContent() {
  useWidgetNotifications();

  const utils = api.useUtils();

  // Listen for settings-changed events from main process to sync mode state
  useEffect(() => {
    const handleSettingsChanged = () => {
      void utils.settings.getModes.invalidate();
    };

    window.electronAPI.on("settings-changed", handleSettingsChanged);
    return () => {
      window.electronAPI.off("settings-changed", handleSettingsChanged);
    };
  }, [utils]);

  return (
    <>
      <ModeSwitchOverlay />
      <FloatingButton />
    </>
  );
}
