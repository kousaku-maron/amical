import React from "react";
import { Button } from "@/components/ui/button";
import { LicenseCard } from "../shared/LicenseCard";

interface WelcomeScreenProps {
  onStart: () => void;
  cardHolderName: string;
}

/**
 * Welcome screen - first screen users see in onboarding
 * Displays welcome copy, license card, and start action.
 */
export function WelcomeScreen({ onStart, cardHolderName }: WelcomeScreenProps) {
  return (
    <div className="relative flex h-full items-center overflow-hidden bg-background px-6 pb-4">
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-0 bg-[radial-gradient(70%_55%_at_16%_18%,rgba(74,104,173,0.28),transparent_70%),radial-gradient(52%_52%_at_82%_20%,rgba(30,155,158,0.17),transparent_72%),linear-gradient(160deg,#10161d_0%,#090d12_58%,#06080b_100%)]"
      />
      <div className="relative mx-auto flex h-[640px] max-h-full w-full max-w-3xl flex-col justify-center">
        <div className="pt-2 text-center">
          <h1 className="text-2xl font-bold tracking-tight text-foreground">
            Welcome to Grizzo
          </h1>
          <p className="mt-2 text-base text-muted-foreground">
            Grizzo is an AI voice input app.
          </p>
        </div>

        <div className="min-h-2 flex-1" />

        <div className="relative w-full self-center max-w-[560px]">
          <LicenseCard cardHolderName={cardHolderName} floating={false} />
        </div>

        <div className="min-h-2 flex-1" />

        <div className="flex justify-center">
          <Button
            onClick={onStart}
            type="button"
            className="h-12 min-w-40 rounded-full px-9 text-lg"
          >
            Start
          </Button>
        </div>
      </div>
    </div>
  );
}
