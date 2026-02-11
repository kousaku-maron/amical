import React from "react";
import { Card } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { OnboardingLayout } from "../shared/OnboardingLayout";
import { NavigationButtons } from "../shared/NavigationButtons";
import { OnboardingMicrophoneSelect } from "../shared/OnboardingMicrophoneSelect";
import { OnboardingShortcutInput } from "../shared/OnboardingShortcutInput";
import { OnboardingHandsFreeInput } from "../shared/OnboardingHandsFreeInput";
import { Settings } from "lucide-react";
interface CompletionScreenProps {
  onComplete: () => void;
  onBack: () => void;
}

/**
 * Completion screen - final screen showing setup is complete
 */
export function CompletionScreen({
  onComplete,
  onBack,
}: CompletionScreenProps) {
  return (
    <OnboardingLayout
      title="Setup Complete!"
      headerSpacingClassName="mb-12"
      topSpacingClassName="pt-8"
      contentFrame={false}
      contentClassName="mx-auto w-full max-w-[760px]"
      className="bg-transparent"
      footer={
        <NavigationButtons
          onComplete={onComplete}
          onBack={onBack}
          showBack={true}
          showNext={false}
          showComplete={true}
          completeLabel="Start"
          showCompleteIcon={false}
        />
      }
    >
      <div className="mx-auto w-full max-w-[760px] space-y-5">
        {/* Quick Settings */}
        <Card className="mx-auto w-full max-w-[540px] p-6">
          <h3 className="mb-4 font-medium flex items-center gap-2">
            <Settings className="h-4 w-4" />
            Quick Settings
          </h3>
          <div className="space-y-4">
            <OnboardingMicrophoneSelect />
            <Separator />
            <OnboardingShortcutInput />
            <Separator />
            <OnboardingHandsFreeInput />
          </div>
        </Card>
      </div>
    </OnboardingLayout>
  );
}
