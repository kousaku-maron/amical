import React, { useState, useCallback, useEffect } from "react";
import { AudioLines, Check } from "lucide-react";
import { IconSparkles } from "@tabler/icons-react";
import { api } from "@/trpc/react";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "@/components/ui/dropdown-menu";
import {
  formatShortcutKey,
  isMacPlatform,
} from "@/components/ui/keyboard-shortcut";

interface WidgetToolbarProps {
  onStartRecording: (e: React.MouseEvent) => void;
  onMenuOpenChange?: (open: boolean) => void;
  onModeChanged?: (modeName: string) => void;
}

const WidgetTooltip: React.FC<{
  text: string;
  shortcut?: string;
  visible: boolean;
}> = ({ text, shortcut, visible }) => {
  if (!visible) return null;
  return (
    <div className="absolute bottom-full left-1/2 -translate-x-1/2 mb-2 px-2.5 py-1 bg-black/90 backdrop-blur-md rounded-md text-white text-[10px] whitespace-nowrap pointer-events-none z-50 animate-in fade-in-0 zoom-in-95 duration-100">
      <span>{text}</span>
      {shortcut && (
        <span className="ml-1.5 text-white/50">{shortcut}</span>
      )}
    </div>
  );
};

export const WidgetToolbar: React.FC<WidgetToolbarProps> = ({
  onStartRecording,
  onMenuOpenChange,
  onModeChanged,
}) => {
  const [hoveredButton, setHoveredButton] = useState<
    "modes" | "grizzo" | null
  >(null);
  const [menuOpen, setMenuOpenInternal] = useState(false);

  const setMenuOpen = useCallback(
    (open: boolean) => {
      setMenuOpenInternal(open);
      onMenuOpenChange?.(open);
    },
    [onMenuOpenChange],
  );

  // Notify parent when unmounted (Radix doesn't call onOpenChange(false) on unmount).
  // Intentionally empty deps: we only need cleanup on unmount, not on prop changes.
  useEffect(() => {
    return () => onMenuOpenChange?.(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- unmount-only cleanup
  }, []);

  const shortcutsQuery = api.settings.getShortcuts.useQuery();
  const useMac = isMacPlatform();
  const formatKeys = (keys?: string[]) =>
    keys?.map((k) => formatShortcutKey(k, useMac)).join("+") ?? "";
  const pttDisplay = formatKeys(shortcutsQuery.data?.pushToTalk);
  const cycleModeDisplay = formatKeys(shortcutsQuery.data?.cycleMode);

  const modesQuery = api.settings.getModes.useQuery();
  const modes = modesQuery.data?.items ?? [];
  const activeModeId = modesQuery.data?.activeModeId;

  const utils = api.useUtils();
  const setActiveMode = api.settings.setActiveMode.useMutation({
    onSuccess: () => {
      utils.settings.getModes.invalidate();
    },
  });

  const handleSelectMode = useCallback(
    async (modeId: string) => {
      if (modeId === activeModeId) return;
      const modeName = modes.find((m) => m.id === modeId)?.name ?? "Mode";
      await setActiveMode.mutateAsync({ modeId });
      onModeChanged?.(modeName);
    },
    [activeModeId, modes, setActiveMode, onModeChanged],
  );

  return (
    <div className="relative flex items-center justify-around h-full w-full">
      {/* Tooltip - centered on the pill */}
      <WidgetTooltip
        text={hoveredButton === "modes" ? "Change mode" : "Start recording"}
        shortcut={
          hoveredButton === "grizzo"
            ? pttDisplay
            : hoveredButton === "modes"
              ? cycleModeDisplay
              : undefined
        }
        visible={hoveredButton !== null && !menuOpen}
      />

      {/* Modes dropdown — invisible full-width trigger for pill-centered positioning */}
      <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen} modal={false}>
        <DropdownMenuTrigger asChild>
          <div className="absolute inset-0 pointer-events-none" tabIndex={-1} />
        </DropdownMenuTrigger>
        <DropdownMenuContent
          side="top"
          align="center"
          sideOffset={8}
          className="min-w-[160px] bg-black/80 backdrop-blur-md border-white/15 shadow-[0px_0px_15px_0px_rgba(0,0,0,0.40)]"
        >
          {modes.map((mode) => {
            const isActive = mode.id === activeModeId;
            return (
              <DropdownMenuItem
                key={mode.id}
                onSelect={() => handleSelectMode(mode.id)}
                className="flex items-center justify-between gap-3 px-3 py-1.5 text-white focus:bg-white/10 focus:text-white"
              >
                <span className="text-xs font-medium truncate">
                  {mode.name}
                </span>
                {isActive && (
                  <Check className="w-3.5 h-3.5 text-white shrink-0" />
                )}
              </DropdownMenuItem>
            );
          })}
        </DropdownMenuContent>
      </DropdownMenu>

      {/* Modes button — manually toggles dropdown */}
      <button
        onClick={() => setMenuOpen(!menuOpen)}
        onMouseEnter={() => setHoveredButton("modes")}
        onMouseLeave={() => setHoveredButton(null)}
        className="flex items-center justify-center w-[28px] h-[28px] rounded-full transition-colors hover:bg-white/15"
        aria-label="Change mode"
      >
        <IconSparkles className="w-[16px] h-[16px] text-white" />
      </button>

      {/* Grizzo button */}
      <button
        onClick={onStartRecording}
        onMouseEnter={() => setHoveredButton("grizzo")}
        onMouseLeave={() => setHoveredButton(null)}
        className="flex items-center justify-center w-[28px] h-[28px] rounded-full transition-colors hover:bg-white/15"
        aria-label="Start recording"
      >
        <AudioLines className="w-[16px] h-[16px] text-white" />
      </button>
    </div>
  );
};
