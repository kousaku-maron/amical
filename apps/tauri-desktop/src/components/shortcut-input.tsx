import React, { useState, useEffect } from "react";
import { Button } from "@/components/ui/button";
import { Pencil, X } from "lucide-react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { api } from "@/trpc/react";
import { toast } from "sonner";

interface ShortcutInputProps {
  value?: string[];
  onChange: (value: string[]) => void;
  isRecordingShortcut?: boolean;
  onRecordingShortcutChange: (recording: boolean) => void;
}

const MODIFIER_KEYS = ["Cmd", "Win", "Ctrl", "Alt", "Shift", "Fn"];
const MAX_KEY_COMBINATION_LENGTH = 4;
const isTauriRuntime =
  typeof window !== "undefined" && "__TAURI__" in window;

const normalizeKey = (event: KeyboardEvent): string | null => {
  const key = event.key;
  if (!key) return null;
  if (key === " ") return "Space";
  if (key === "Meta") return window.electronAPI?.platform === "darwin" ? "Cmd" : "Win";
  if (key === "Control") return "Ctrl";
  if (key === "Alt") return "Alt";
  if (key === "Shift") return "Shift";
  if (key === "Escape") return "Escape";
  if (key === "Backspace") return "Backspace";
  if (key === "Delete") return "Delete";
  if (key === "Tab") return "Tab";
  if (key === "Enter") return "Enter";
  if (key.startsWith("Arrow")) return key.replace("Arrow", "");
  if (/^F\\d+$/.test(key)) return key;
  if (key.length === 1) return key.toUpperCase();
  return key;
};

type ValidationResult = {
  valid: boolean;
  shortcut?: string[];
  error?: string;
};

/**
 * Basic format validation only - business logic validation happens on backend
 */
function validateShortcutFormat(keys: string[]): ValidationResult {
  if (keys.length === 0) {
    return { valid: false, error: "No keys detected" };
  }

  if (keys.length > MAX_KEY_COMBINATION_LENGTH) {
    return {
      valid: false,
      error: `Too many keys - use ${MAX_KEY_COMBINATION_LENGTH} or fewer`,
    };
  }

  const modifierKeys = keys.filter((key) => MODIFIER_KEYS.includes(key));
  const regularKeys = keys.filter((key) => !MODIFIER_KEYS.includes(key));

  // Return array format: modifiers first, then regular keys
  return {
    valid: true,
    shortcut: [...modifierKeys, ...regularKeys],
  };
}

function RecordingDisplay({
  activeKeys,
  onCancel,
}: {
  activeKeys: string[];
  onCancel: () => void;
}) {
  return (
    <div
      className="inline-flex items-center gap-2 px-3 py-1 bg-muted rounded-md ring-2 ring-primary"
      tabIndex={0}
    >
      {activeKeys.length > 0 ? (
        <div className="flex items-center gap-1">
          {activeKeys.map((key, index) => (
            <kbd
              key={index}
              className="px-1.5 py-0.5 text-xs bg-background rounded border"
            >
              {key}
            </kbd>
          ))}
        </div>
      ) : (
        <span className="text-sm text-muted-foreground">Press keys...</span>
      )}
      <Button
        variant="ghost"
        size="sm"
        className="h-6 w-6 p-0"
        onClick={onCancel}
      >
        <X className="h-3 w-3" />
      </Button>
    </div>
  );
}

function ShortcutDisplay({
  value,
  onEdit,
}: {
  value?: string[];
  onEdit: () => void;
}) {
  // Format array as display string (e.g., ["Fn", "Space"] -> "Fn+Space")
  const displayValue = value?.length ? value.join("+") : undefined;

  return (
    <>
      {displayValue && (
        <kbd
          onClick={onEdit}
          className="inline-flex items-center px-3 py-1 bg-muted hover:bg-muted/70 rounded-md text-sm font-mono cursor-pointer transition-colors"
        >
          {displayValue}
        </kbd>
      )}
      <Button
        variant="ghost"
        size="sm"
        className="h-6 w-6 p-0"
        onClick={onEdit}
      >
        <Pencil className="h-3 w-3" />
      </Button>
    </>
  );
}

export function ShortcutInput({
  value,
  onChange,
  isRecordingShortcut = false,
  onRecordingShortcutChange,
}: ShortcutInputProps) {
  const [activeKeys, setActiveKeys] = useState<string[]>([]);
  const setRecordingStateMutation =
    api.settings.setShortcutRecordingState.useMutation();

  const handleStartRecording = () => {
    onRecordingShortcutChange(true);
    setRecordingStateMutation.mutate(true);
  };

  const handleCancelRecording = () => {
    onRecordingShortcutChange(false);
    setActiveKeys([]);
    setRecordingStateMutation.mutate(false);
  };

  // Subscribe to key events when recording
  // Note: activeKeys closure is fresh on each render because useSubscription
  // updates its callback reference, so previousKeys correctly captures the
  // previous state value when onData fires.
  api.settings.activeKeysUpdates.useSubscription(undefined, {
    enabled: isRecordingShortcut && !isTauriRuntime,
    onData: (keys: string[]) => {
      const previousKeys = activeKeys;
      setActiveKeys(keys);

      // When any key is released, validate the combination
      if (previousKeys.length > 0 && keys.length < previousKeys.length) {
        const result = validateShortcutFormat(previousKeys);

        if (result.valid && result.shortcut) {
          // Basic format is valid - let parent handle backend validation
          onChange(result.shortcut);
        } else {
          toast.error(result.error || "Invalid key combination");
        }

        onRecordingShortcutChange(false);
        setRecordingStateMutation.mutate(false);
      }
    },
    onError: (error) => {
      console.error("Error subscribing to active keys", error);
    },
  });

  // Reset state when recording starts
  useEffect(() => {
    if (isRecordingShortcut) {
      setActiveKeys([]);
    }
  }, [isRecordingShortcut]);

  useEffect(() => {
    if (!isRecordingShortcut || !isTauriRuntime) {
      return;
    }

    let previousKeys: string[] = [];
    const pressedKeys = new Set<string>();

    const updateKeys = () => {
      const keys = Array.from(pressedKeys);
      setActiveKeys(keys);
      previousKeys = keys;
    };

    const handleKeyDown = (event: KeyboardEvent) => {
      const key = normalizeKey(event);
      if (!key) return;
      if (event.repeat) return;
      event.preventDefault();
      pressedKeys.add(key);
      updateKeys();
    };

    const handleKeyUp = (event: KeyboardEvent) => {
      const key = normalizeKey(event);
      if (!key) return;
      event.preventDefault();
      pressedKeys.delete(key);
      const nextKeys = Array.from(pressedKeys);
      setActiveKeys(nextKeys);

      if (previousKeys.length > 0 && nextKeys.length < previousKeys.length) {
        const result = validateShortcutFormat(previousKeys);

        if (result.valid && result.shortcut) {
          onChange(result.shortcut);
        } else {
          toast.error(result.error || "Invalid key combination");
        }

        onRecordingShortcutChange(false);
        setRecordingStateMutation.mutate(false);
      }

      previousKeys = nextKeys;
    };

    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);

    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
    };
  }, [isRecordingShortcut, onChange, onRecordingShortcutChange, setRecordingStateMutation]);

  return (
    <TooltipProvider>
      <div className="inline-flex items-center gap-2">
        {isRecordingShortcut ? (
          <RecordingDisplay
            activeKeys={activeKeys}
            onCancel={handleCancelRecording}
          />
        ) : (
          <ShortcutDisplay value={value} onEdit={handleStartRecording} />
        )}
      </div>
    </TooltipProvider>
  );
}
