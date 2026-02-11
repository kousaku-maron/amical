import React, { useEffect, useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { OnboardingLayout } from "../shared/OnboardingLayout";
import { NavigationButtons } from "../shared/NavigationButtons";
import { cn } from "@/lib/utils";
import { Check, Download, AlertCircle } from "lucide-react";
import { toast } from "sonner";
import { api } from "@/trpc/react";
import type { DownloadProgress } from "@/constants/models";

interface ModelSelectionScreenProps {
  onNext: () => void;
  onBack: () => void;
}

/**
 * Model selection screen - local model setup
 */
export function ModelSelectionScreen({
  onNext,
  onBack,
}: ModelSelectionScreenProps) {
  const [downloadProgress, setDownloadProgress] = useState<
    Record<string, DownloadProgress>
  >({});
  const [error, setError] = useState<string | null>(null);

  const availableModelsQuery = api.models.getAvailableModels.useQuery();
  const downloadedModelsQuery = api.models.getDownloadedModels.useQuery();
  const activeDownloadsQuery = api.models.getActiveDownloads.useQuery();
  const downloadModelMutation = api.models.downloadModel.useMutation();
  const setSelectedModelMutation = api.models.setSelectedModel.useMutation();
  const utils = api.useUtils();

  const offlineModels = useMemo(() => {
    return (availableModelsQuery.data || []).filter(
      (model) => model.setup === "offline",
    );
  }, [availableModelsQuery.data]);

  const recommendedModelId = "whisper-large-v3-turbo";
  const preferredOrder = useMemo(
    () => [
      recommendedModelId,
      "whisper-large-v3",
      "whisper-large-v1",
      "whisper-medium",
      "whisper-small",
      "whisper-base",
      "whisper-tiny",
    ],
    [recommendedModelId],
  );

  const downloadedModels = downloadedModelsQuery.data || {};
  const autoSelectedModelId = useMemo(() => {
    const downloadedIds = Object.keys(downloadedModels);
    if (downloadedIds.length === 0) return null;
    for (const candidateId of preferredOrder) {
      if (downloadedModels[candidateId]) return candidateId;
    }
    return downloadedIds[0];
  }, [downloadedModels, preferredOrder]);

  useEffect(() => {
    if (activeDownloadsQuery.data) {
      const progressMap: Record<string, DownloadProgress> = {};
      activeDownloadsQuery.data.forEach((download) => {
        progressMap[download.modelId] = download;
      });
      setDownloadProgress(progressMap);
    }
  }, [activeDownloadsQuery.data]);

  api.models.onDownloadProgress.useSubscription(undefined, {
    onData: ({ modelId, progress }) => {
      setDownloadProgress((prev) => ({ ...prev, [modelId]: progress }));
    },
  });

  api.models.onDownloadComplete.useSubscription(undefined, {
    onData: ({ modelId }) => {
      setDownloadProgress((prev) => {
        const next = { ...prev };
        delete next[modelId];
        return next;
      });
      utils.models.getDownloadedModels.invalidate();
      utils.models.getActiveDownloads.invalidate();
    },
  });

  api.models.onDownloadError.useSubscription(undefined, {
    onData: ({ modelId, error: message }) => {
      setDownloadProgress((prev) => {
        const next = { ...prev };
        delete next[modelId];
        return next;
      });
      setError(message || "Download failed");
      utils.models.getActiveDownloads.invalidate();
    },
  });

  api.models.onDownloadCancelled.useSubscription(undefined, {
    onData: ({ modelId }) => {
      setDownloadProgress((prev) => {
        const next = { ...prev };
        delete next[modelId];
        return next;
      });
      utils.models.getActiveDownloads.invalidate();
    },
  });

  const handleContinue = () => {
    if (!autoSelectedModelId) {
      toast.error("Please download a model");
      return;
    }

    setSelectedModelMutation.mutate(
      { modelId: autoSelectedModelId },
      {
        onSuccess: () => {
          onNext();
        },
        onError: (err) => {
          const message = err instanceof Error ? err.message : String(err);
          toast.error(`Failed to select model: ${message}`);
        },
      },
    );
  };

  const canContinue = Boolean(autoSelectedModelId);
  const handleDownload = (modelId: string) => {
    setError(null);
    downloadModelMutation.mutateAsync({ modelId }).catch((err) => {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
    });
  };

  const renderModelCard = (
    model: (typeof offlineModels)[number],
    isRecommended: boolean,
  ) => {
    const progress = downloadProgress[model.id];
    const downloaded = Boolean(downloadedModels[model.id]);
    const isDownloading = Boolean(progress);

    return (
      <div
        key={model.id}
        className={cn(
          "rounded-lg border bg-black/20 p-4 shadow-[0_12px_30px_rgba(0,0,0,0.25)] backdrop-blur-sm",
          isRecommended ? "border-primary/65 bg-black/30" : "border-white/15",
        )}
      >
        <div className="flex items-center justify-between gap-4">
          <div>
            <div className="flex items-center gap-2">
              <p className="text-sm font-medium">{model.name}</p>
              {isRecommended && (
                <Badge
                  variant="secondary"
                  className="border border-primary/35 bg-primary/10 text-[10px] text-primary"
                >
                  Recommended
                </Badge>
              )}
            </div>
            <p className="text-xs text-muted-foreground">
              {model.sizeFormatted || model.modelSize || ""}
            </p>
          </div>

          {downloaded ? (
            <Button
              type="button"
              size="icon"
              variant="ghost"
              disabled
              className="text-green-600 disabled:opacity-100"
              title="Downloaded"
              aria-label="Downloaded"
            >
              <Check className="h-4 w-4" />
            </Button>
          ) : isDownloading ? (
            <Button
              type="button"
              size="icon"
              variant="ghost"
              disabled
              className="disabled:opacity-100"
              title="Downloading"
              aria-label="Downloading"
            >
              <Download className="h-4 w-4 text-muted-foreground" />
            </Button>
          ) : isRecommended ? (
            <Button
              type="button"
              size="sm"
              className="gap-2"
              title="Download"
              aria-label="Download"
              onClick={(event) => {
                event.stopPropagation();
                handleDownload(model.id);
              }}
            >
              <Download className="h-4 w-4" />
              Download
            </Button>
          ) : (
            <Button
              type="button"
              size="icon"
              variant="ghost"
              title="Download"
              aria-label="Download"
              onClick={(event) => {
                event.stopPropagation();
                handleDownload(model.id);
              }}
            >
              <Download className="h-4 w-4 text-muted-foreground" />
            </Button>
          )}
        </div>

        {progress && (
          <div className="mt-3 space-y-2">
            <div className="flex items-center gap-3">
              <Download
                className={cn(
                  "h-4 w-4",
                  isRecommended ? "text-primary" : "text-muted-foreground",
                )}
              />
              <div className="flex-1">
                <Progress value={progress.progress} className="h-2" />
              </div>
              <span className="text-xs font-medium">{progress.progress}%</span>
            </div>
            <div className="text-xs text-muted-foreground">
              {(progress.bytesDownloaded / (1024 * 1024)).toFixed(1)} /
              {(progress.totalBytes / (1024 * 1024)).toFixed(1)} MB
            </div>
          </div>
        )}
      </div>
    );
  };

  return (
    <OnboardingLayout
      title="Set Up Speech Model"
      subtitle="Download a Whisper offline model to enable transcription"
      headerSpacingClassName="mb-12"
      topSpacingClassName="pt-8"
      contentFrame={false}
      contentClassName="mx-auto w-full max-w-[760px]"
      className="bg-transparent"
      footer={
        <NavigationButtons
          onBack={onBack}
          onNext={handleContinue}
          disableNext={!canContinue}
          nextLabel={canContinue ? "Next" : "Download a model first"}
        />
      }
    >
      <div className="mx-auto w-full max-w-[760px] space-y-4">
        {/* Offline Model List */}
        <div className="mx-auto w-full max-w-[540px] space-y-4">
          {offlineModels
            .slice()
            .sort((a, b) => {
              if (a.id === recommendedModelId) return -1;
              if (b.id === recommendedModelId) return 1;
              return 0;
            })
            .map((model) =>
              renderModelCard(model, model.id === recommendedModelId),
            )}
        </div>

        {error && (
          <div className="mx-auto flex w-full max-w-[540px] items-center gap-2 rounded-lg bg-destructive/10 p-3 text-sm text-destructive">
            <AlertCircle className="h-4 w-4" />
            {error}
          </div>
        )}
      </div>
    </OnboardingLayout>
  );
}
