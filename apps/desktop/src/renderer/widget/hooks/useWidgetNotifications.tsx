import { toast } from "sonner";
import { api } from "@/trpc/react";
import { useAudioDevices } from "@/hooks/useAudioDevices";
import {
  WIDGET_NOTIFICATION_TIMEOUT,
  getNotificationDescription,
  type WidgetNotificationAction,
} from "@/types/widget-notification";
import { WidgetToast } from "../components/WidgetToast";
import { useMouseEvents } from "../contexts/MouseEventsContext";

export const useWidgetNotifications = () => {
  const navigateMainWindow = api.widget.navigateMainWindow.useMutation();
  const { acquire, release } = useMouseEvents();
  const { data: settings } = api.settings.getSettings.useQuery();
  const { defaultDeviceName } = useAudioDevices();

  // Get effective mic name: preferred from settings, or system default
  const getEffectiveMicName = () => {
    return settings?.recording?.preferredMicrophoneName || defaultDeviceName;
  };

  const handleActionClick = async (action: WidgetNotificationAction) => {
    if (action.navigateTo) {
      navigateMainWindow.mutate({ route: action.navigateTo });
    } else if (action.externalUrl) {
      await window.electronAPI.openExternal(action.externalUrl);
    }
    // release is handled by onDismiss via toast.dismiss() below
  };

  api.recording.widgetNotifications.useSubscription(undefined, {
    onData: (notification) => {
      const micName = getEffectiveMicName();
      const description = getNotificationDescription(
        notification.type,
        micName,
      );

      toast.custom(
        (toastId) => (
          <div
            onMouseEnter={acquire}
            onMouseLeave={release}
            style={{ pointerEvents: "auto" }}
          >
            <WidgetToast
              title={notification.title}
              description={description}
              primaryAction={notification.primaryAction}
              secondaryAction={notification.secondaryAction}
              onActionClick={(action) => {
                handleActionClick(action);
                toast.dismiss(toastId);
              }}
            />
          </div>
        ),
        {
          unstyled: true,
          duration: WIDGET_NOTIFICATION_TIMEOUT,
          onDismiss: release,
          onAutoClose: release,
        },
      );
    },
    onError: (error) => {
      console.error("Widget notification subscription error:", error);
    },
  });
};
