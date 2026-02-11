import { useEffect, useState } from "react";
import { Label } from "@/components/ui/label";
import { ShortcutInput } from "@/components/shortcut-input";
import { api } from "@/trpc/react";
import { toast } from "sonner";

/**
 * Hands-free shortcut input for onboarding
 * Wraps ShortcutInput with label and handles data fetching/saving
 */
export function OnboardingHandsFreeInput() {
  const [toggleRecordingShortcut, setToggleRecordingShortcut] = useState<
    string[]
  >([]);
  const [isRecording, setIsRecording] = useState(false);

  const utils = api.useUtils();
  const shortcutsQuery = api.settings.getShortcuts.useQuery();
  const setShortcutMutation = api.settings.setShortcut.useMutation({
    onSuccess: () => {
      utils.settings.getShortcuts.invalidate();
    },
    onError: (error) => {
      toast.error(error.message);
      // Revert to saved value
      utils.settings.getShortcuts.invalidate();
    },
  });

  useEffect(() => {
    if (shortcutsQuery.data) {
      setToggleRecordingShortcut(shortcutsQuery.data.toggleRecording);
    }
  }, [shortcutsQuery.data]);

  const handleShortcutChange = (shortcut: string[]) => {
    setToggleRecordingShortcut(shortcut);
    setShortcutMutation.mutate({
      type: "toggleRecording",
      shortcut,
    });
  };

  return (
    <div className="flex items-center justify-between">
      <div>
        <Label className="text-base font-semibold text-foreground">
          Hands-free mode
        </Label>
        <p className="text-xs text-muted-foreground mt-1">
          Press once to start and press again to stop dictation
        </p>
      </div>
      <div className="min-w-[200px] flex justify-end">
        <ShortcutInput
          value={toggleRecordingShortcut}
          onChange={handleShortcutChange}
          isRecordingShortcut={isRecording}
          onRecordingShortcutChange={setIsRecording}
        />
      </div>
    </div>
  );
}
