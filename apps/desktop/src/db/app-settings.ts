/**
 * Application Settings Management
 *
 * This module manages app settings with a section-based approach:
 * - Settings are organized into top-level sections (ui, transcription, recording, etc.)
 * - Each section is treated as an atomic unit - updates replace the entire section
 * - No deep merging is performed within sections to avoid complexity
 *
 * When updating settings:
 * - To update a single field, fetch the current section, modify it, and save the complete section
 * - The SettingsService handles this pattern correctly for all methods
 * - Direct calls to updateAppSettings should pass complete sections
 */

import { eq } from "drizzle-orm";
import { db } from ".";
import {
  appSettings,
  type NewAppSettings,
  type AppSettingsData,
  type ModeConfig,
} from "./schema";
import { isMacOS } from "../utils/platform";

// Current baseline settings schema version
// Bump this when adding migrations to migrateSettingsData()
const CURRENT_SETTINGS_VERSION = 2;

function createDefaultMode(
  settings: Pick<AppSettingsData, "dictation" | "formatterConfig"> = {},
): ModeConfig {
  const now = new Date().toISOString();
  return {
    id: "default",
    name: "Default",
    isDefault: true,
    dictation: settings.dictation ?? {
      autoDetectEnabled: true,
      selectedLanguage: "en",
    },
    formatterConfig: settings.formatterConfig ?? { enabled: false },
    customInstructions: undefined,
    createdAt: now,
    updatedAt: now,
  };
}

// Singleton ID for app settings (we only have one settings record)
const SETTINGS_ID = 1;

// Platform-specific default shortcuts (array format)
const getDefaultShortcuts = () => {
  if (isMacOS()) {
    return {
      pushToTalk: ["Fn"],
      toggleRecording: ["Fn", "Space"],
      cycleMode: ["Alt", "Shift", "K"],
    };
  } else {
    // Windows and Linux
    return {
      pushToTalk: ["Ctrl", "Win"],
      toggleRecording: ["Ctrl", "Win", "Space"],
      cycleMode: ["Alt", "Shift", "K"],
    };
  }
};

// Default settings
const defaultSettings: AppSettingsData = {
  formatterConfig: {
    enabled: false,
  },
  ui: {
    theme: "dark",
  },
  preferences: {
    launchAtLogin: true,
    showWidgetWhileInactive: true,
    showInDock: true,
  },
  transcription: {
    language: "en",
    autoTranscribe: true,
    confidenceThreshold: 0.8,
    enablePunctuation: true,
    enableTimestamps: false,
  },
  recording: {
    defaultFormat: "wav",
    sampleRate: 16000,
    autoStopSilence: true,
    silenceThreshold: 3,
    maxRecordingDuration: 60,
  },
  shortcuts: getDefaultShortcuts(),
  modelProvidersConfig: {
    defaultSpeechModel: "",
  },
  modes: {
    items: [createDefaultMode()],
    activeModeId: "default",
  },
};

// Get all app settings
export async function getAppSettings(): Promise<AppSettingsData> {
  const result = await db
    .select()
    .from(appSettings)
    .where(eq(appSettings.id, SETTINGS_ID));

  if (result.length === 0) {
    // Create default settings if none exist
    await createDefaultSettings();
    return defaultSettings;
  }

  const record = result[0];
  return record.data;
}

// Update app settings (shallow merge at top level only)
// IMPORTANT: When updating a section, always pass the complete section.
// This function does NOT deep merge nested objects.
export async function updateAppSettings(
  newSettings: Partial<AppSettingsData>,
): Promise<AppSettingsData> {
  const currentSettings = await getAppSettings();

  // Simple shallow merge - each top-level section is replaced entirely
  const mergedSettings: AppSettingsData = {
    ...currentSettings,
    ...newSettings,
  };

  const now = new Date();

  await db
    .update(appSettings)
    .set({
      data: mergedSettings,
      updatedAt: now,
    })
    .where(eq(appSettings.id, SETTINGS_ID));

  return mergedSettings;
}

// Replace all app settings (complete override)
export async function replaceAppSettings(
  newSettings: AppSettingsData,
): Promise<AppSettingsData> {
  const now = new Date();

  await db
    .update(appSettings)
    .set({
      data: newSettings,
      updatedAt: now,
    })
    .where(eq(appSettings.id, SETTINGS_ID));

  return newSettings;
}

// Get a specific setting section
export async function getSettingsSection<K extends keyof AppSettingsData>(
  section: K,
): Promise<AppSettingsData[K]> {
  const settings = await getAppSettings();
  return settings[section];
}

//! Update a specific setting section (replaces the entire section)
//! IMPORTANT: This replaces the entire section with newData.
//! Make sure to pass the complete section data, not partial updates.
export async function updateSettingsSection<K extends keyof AppSettingsData>(
  section: K,
  newData: AppSettingsData[K],
): Promise<AppSettingsData> {
  return await updateAppSettings({
    [section]: newData,
  } as Partial<AppSettingsData>);
}

// Reset settings to defaults
export async function resetAppSettings(): Promise<AppSettingsData> {
  return await replaceAppSettings(defaultSettings);
}

// Create default settings (internal helper)
async function createDefaultSettings(): Promise<void> {
  const now = new Date();

  const newSettings: NewAppSettings = {
    id: SETTINGS_ID,
    data: defaultSettings,
    version: CURRENT_SETTINGS_VERSION,
    createdAt: now,
    updatedAt: now,
  };

  await db.insert(appSettings).values(newSettings);
}

/**
 * Run data migrations on the JSON settings blob.
 * Called once at startup after DB schema migrations complete.
 * Uses the `version` column to track which migrations have been applied.
 */
export async function migrateSettingsData(): Promise<void> {
  const result = await db
    .select()
    .from(appSettings)
    .where(eq(appSettings.id, SETTINGS_ID));

  if (result.length === 0) return; // No record yet; defaults will be used

  const record = result[0];
  const version = record.version ?? 1;

  if (version >= CURRENT_SETTINGS_VERSION) return; // Already up to date

  const data = { ...record.data };

  // v1 → v2: Add cycleMode default to existing shortcuts
  if (version < 2) {
    if (data.shortcuts && data.shortcuts.cycleMode === undefined) {
      data.shortcuts = {
        ...data.shortcuts,
        cycleMode: getDefaultShortcuts().cycleMode,
      };
    }
  }

  await db
    .update(appSettings)
    .set({ data, version: CURRENT_SETTINGS_VERSION, updatedAt: new Date() })
    .where(eq(appSettings.id, SETTINGS_ID));
}

// Export default settings for reference
export { defaultSettings };
