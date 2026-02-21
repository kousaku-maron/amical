import React from "react";
import { Toaster } from "@/components/ui/sonner";

/**
 * Wrapper for Toaster in the widget window.
 * Toast notifications are display-only (pointer-events-none)
 * to avoid interfering with setIgnoreMouseEvents managed by FloatingButton.
 */
export const ToasterWrapper: React.FC = () => {
  return (
    <div style={{ pointerEvents: "none" }}>
      <Toaster position="bottom-center" />
    </div>
  );
};
