import React from "react";
import { cn } from "@/lib/utils";

interface OnboardingLayoutProps {
  children: React.ReactNode;
  footer?: React.ReactNode;
  title?: string;
  titleIcon?: React.ReactNode;
  subtitle?: string;
  headerSpacingClassName?: string;
  topSpacingClassName?: string;
  contentFrame?: boolean;
  contentVerticalCenter?: boolean;
  contentClassName?: string;
  className?: string;
}

/**
 * Shared layout component for all onboarding screens
 * Provides consistent structure and styling
 */
export function OnboardingLayout({
  children,
  footer,
  title,
  titleIcon,
  subtitle,
  headerSpacingClassName = "mb-4",
  topSpacingClassName,
  contentFrame = true,
  contentVerticalCenter = false,
  contentClassName,
  className,
}: OnboardingLayoutProps) {
  return (
    <div
      className={cn(
        "flex h-full flex-col items-center bg-background px-6 py-4",
        className,
      )}
    >
      {/* Scrollable content area */}
      <div
        className={cn(
          "flex-1 w-full max-w-3xl overflow-auto",
          contentVerticalCenter && "flex flex-col",
          topSpacingClassName,
        )}
      >
        <div className={cn(contentVerticalCenter && "my-auto")}>
          {/* Header */}
          {(title || subtitle) && (
            <div className={cn("text-center", headerSpacingClassName)}>
              {title && (
                <h1 className="flex items-center justify-center gap-2 text-2xl font-bold tracking-tight text-foreground">
                  {titleIcon}
                  {title}
                </h1>
              )}
              {subtitle && (
                <p className="mt-2 text-base text-muted-foreground">
                  {subtitle}
                </p>
              )}
            </div>
          )}

          {/* Content */}
          <div
            className={cn(
              contentFrame
                ? "rounded-lg border bg-card p-4 shadow-sm"
                : "p-0 border-0 bg-transparent shadow-none",
              contentClassName,
            )}
          >
            {children}
          </div>
        </div>
      </div>

      {/* Footer - pinned to bottom */}
      {footer && <div className="w-full max-w-3xl pt-4 mt-auto">{footer}</div>}
    </div>
  );
}
