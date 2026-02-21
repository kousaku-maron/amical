import React, { useState, useRef, useEffect } from "react";
import { Square } from "lucide-react";
import { IconSparkles } from "@tabler/icons-react";
import { Waveform } from "@/components/Waveform";
import { useRecording } from "@/hooks/useRecording";
import { api } from "@/trpc/react";
import { WidgetToolbar } from "./WidgetToolbar";

const NUM_WAVEFORM_BARS = 6;

const StopButton: React.FC<{ onClick: (e: React.MouseEvent) => void }> = ({
  onClick,
}) => (
  <button
    onClick={onClick}
    className="flex items-center justify-center w-[28px] h-[28px] rounded transition-colors"
    aria-label="Stop recording"
  >
    <Square className="w-[16px] h-[16px] text-red-500 fill-red-500" />
  </button>
);

const ProcessingIndicator: React.FC = () => (
  <div className="flex gap-[5px] items-center justify-center flex-1 h-9">
    <div className="w-[5px] h-[5px] bg-blue-500 rounded-full animate-bounce [animation-delay:-0.3s]" />
    <div className="w-[5px] h-[5px] bg-blue-500 rounded-full animate-bounce [animation-delay:-0.15s]" />
    <div className="w-[5px] h-[5px] bg-blue-500 rounded-full animate-bounce" />
  </div>
);

const WaveformVisualization: React.FC<{
  isRecording: boolean;
  voiceDetected: boolean;
}> = ({ isRecording, voiceDetected }) => (
  <>
    {Array.from({ length: NUM_WAVEFORM_BARS }).map((_, index) => (
      <Waveform
        key={index}
        index={index}
        isRecording={isRecording}
        voiceDetected={voiceDetected}
        baseHeight={60}
        silentHeight={20}
      />
    ))}
  </>
);

export const FloatingButton: React.FC = () => {
  const [isHovered, setIsHovered] = useState(false);
  const [isMenuOpen, setIsMenuOpen] = useState(false);
  const [modeNotification, setModeNotification] = useState<string | null>(null);
  const clickTimeRef = useRef<number | null>(null);

  const setIgnoreMouseEvents = api.widget.setIgnoreMouseEvents.useMutation();

  const { recordingStatus, stopRecording, voiceDetected, startRecording } =
    useRecording();
  const isRecording =
    recordingStatus.state === "recording" ||
    recordingStatus.state === "starting";
  const isStopping = recordingStatus.state === "stopping";
  const isHandsFreeMode = recordingStatus.mode === "hands-free";

  useEffect(() => {
    if (recordingStatus.state === "recording" && clickTimeRef.current) {
      clickTimeRef.current = null;
    }
  }, [recordingStatus.state]);

  // Centralized setIgnoreMouseEvents management:
  // Window receives mouse events when hovered OR menu is open.
  // Re-enable click-through immediately when neither is true.
  useEffect(() => {
    setIgnoreMouseEvents.mutate({ ignore: !(isHovered || isMenuOpen) });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isHovered, isMenuOpen]);

  const handleMouseEnter = () => setIsHovered(true);
  const handleMouseLeave = () => setIsHovered(false);

  const handleMenuOpenChange = (open: boolean) => {
    setIsMenuOpen(open);
    if (!open) setIsHovered(false);
  };

  const handleModeChanged = (modeName: string) => {
    setModeNotification(modeName);
  };

  // Auto-dismiss mode notification
  useEffect(() => {
    if (!modeNotification) return;
    const timer = setTimeout(() => setModeNotification(null), 2000);
    return () => clearTimeout(timer);
  }, [modeNotification]);

  const handleButtonClick = async (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    clickTimeRef.current = performance.now();

    if (recordingStatus.state === "idle") {
      await startRecording();
    } else {
      clickTimeRef.current = null;
    }
  };

  const handleStopClick = async (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    await stopRecording();
  };

  const expanded = isRecording || isStopping || isHovered || isMenuOpen;
  const isIdle = !isRecording && !isStopping;

  const renderWidgetContent = () => {
    if (!expanded) return null;

    if (isStopping) {
      return <ProcessingIndicator />;
    }

    if (isHandsFreeMode && isRecording) {
      return (
        <>
          <div className="justify-center items-center flex flex-1 gap-1">
            <WaveformVisualization
              isRecording={isRecording}
              voiceDetected={voiceDetected}
            />
          </div>
          <div className="h-full items-center flex mr-2">
            <StopButton onClick={handleStopClick} />
          </div>
        </>
      );
    }

    if (isRecording) {
      return (
        <div className="justify-center items-center flex flex-1 gap-1 h-full w-full">
          <WaveformVisualization
            isRecording={isRecording}
            voiceDetected={voiceDetected}
          />
        </div>
      );
    }

    if (isIdle) {
      return (
        <WidgetToolbar
          onStartRecording={handleButtonClick}
          onMenuOpenChange={handleMenuOpenChange}
          onModeChanged={handleModeChanged}
        />
      );
    }

    return null;
  };

  return (
    <div
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
      className="relative"
      style={{ pointerEvents: "auto" }}
    >
      {/* Mode notification - centered above pill */}
      {modeNotification && (
        <div className="absolute bottom-full left-1/2 -translate-x-1/2 mb-2 pointer-events-none z-50 animate-in fade-in-0 zoom-in-95 duration-150">
          <div className="flex items-center gap-2.5 px-4 py-2.5 bg-black/90 backdrop-blur-md rounded-xl text-white text-sm whitespace-nowrap shadow-[0px_0px_15px_0px_rgba(0,0,0,0.40)] ring-[1px] ring-white/15">
            <IconSparkles className="w-4 h-4 text-white/70 shrink-0" />
            <span>{modeNotification} mode active</span>
          </div>
        </div>
      )}

      {/* Pill */}
      <div
        className={`
          transition-all duration-200 ease-in-out
          ${expanded ? "h-[40px] w-[120px] bg-black" : "h-[10px] w-[56px] bg-black/70"}
          rounded-[24px] backdrop-blur-md ring-[1px] ring-black/60 shadow-[0px_0px_15px_0px_rgba(0,0,0,0.40)]
          before:content-[''] before:absolute before:inset-[1px] before:rounded-[23px] before:outline before:outline-white/15 before:pointer-events-none
          mb-2 cursor-pointer select-none
        `}
      >
        {expanded && (
          <div className="flex gap-[2px] h-full w-full justify-between">
            {renderWidgetContent()}
          </div>
        )}
      </div>
    </div>
  );
};
