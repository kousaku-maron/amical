import { autoUpdater } from "electron";
import { EventEmitter } from "events";
import { logger } from "../logger";

export class AutoUpdaterService extends EventEmitter {
  private updateDownloaded = false;

  constructor() {
    super();
    autoUpdater.on("update-downloaded", () => {
      logger.updater.info(
        "Update downloaded and ready to install on next restart",
      );
      this.updateDownloaded = true;
    });
  }

  // These methods are kept for compatibility with existing code
  // update-electron-app handles the actual update logic

  async checkForUpdates(userInitiated = false): Promise<void> {
    logger.updater.info(
      "Update check requested, handled by update-electron-app",
    );
  }

  async checkForUpdatesAndNotify(): Promise<void> {
    logger.updater.info(
      "Background update check requested, handled by update-electron-app",
    );
  }

  isCheckingForUpdate(): boolean {
    return false; // Handled by update-electron-app
  }

  isUpdateAvailable(): boolean {
    return false; // Handled by update-electron-app
  }

  isUpdateDownloaded(): boolean {
    return this.updateDownloaded;
  }

  async downloadUpdate(): Promise<void> {
    logger.updater.info(
      "Download update requested, handled by update-electron-app",
    );
  }

  quitAndInstall(): void {
    if (!this.updateDownloaded) {
      logger.updater.warn("No update downloaded, ignoring quitAndInstall");
      return;
    }
    logger.updater.info("Quit and install requested");
    autoUpdater.quitAndInstall();
  }
}
