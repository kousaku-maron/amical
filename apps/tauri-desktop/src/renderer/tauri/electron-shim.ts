import type { ElectronAPI } from "@/types/electron-api";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const isTauriRuntime = () =>
  typeof window !== "undefined" && "__TAURI__" in window;

const detectPlatform = (): NodeJS.Platform => {
  if (typeof navigator === "undefined") return "linux";
  const ua = navigator.userAgent.toLowerCase();
  if (ua.includes("mac")) return "darwin";
  if (ua.includes("win")) return "win32";
  if (ua.includes("linux")) return "linux";
  return "linux";
};

const listenerMap = new Map<
  string,
  Array<{ handler: (...args: any[]) => void; unlisten: Promise<() => void> }>
>();

const attachListener = (channel: string, handler: (...args: any[]) => void) => {
  if (!isTauriRuntime()) return;
  const unlisten = listen(channel, (event) => handler(event.payload));
  const entries = listenerMap.get(channel) ?? [];
  entries.push({ handler, unlisten });
  listenerMap.set(channel, entries);
  return () => {
    void unlisten.then((stop) => stop());
  };
};

const detachListener = (channel: string, handler: (...args: any[]) => void) => {
  const entries = listenerMap.get(channel);
  if (!entries) return;
  const remaining: typeof entries = [];
  for (const entry of entries) {
    if (entry.handler === handler) {
      void entry.unlisten.then((stop) => stop());
    } else {
      remaining.push(entry);
    }
  }
  if (remaining.length === 0) {
    listenerMap.delete(channel);
  } else {
    listenerMap.set(channel, remaining);
  }
};

const safeInvoke = async <T>(command: string, payload?: Record<string, any>) => {
  if (!isTauriRuntime()) {
    console.warn(`[tauri] invoke skipped: ${command}`);
    return undefined as T | undefined;
  }
  try {
    return (await invoke<T>(command, payload ?? {})) as T;
  } catch (error) {
    console.warn(`[tauri] invoke failed: ${command}`, error);
    return undefined as T | undefined;
  }
};

const baseConsole = () => (console as Console).original ?? console;

const createScopedLogger = (scope: string) => {
  const prefix = `[${scope}]`;
  const logger = baseConsole();
  return {
    info: (...args: any[]) => logger.info(prefix, ...args),
    warn: (...args: any[]) => logger.warn(prefix, ...args),
    error: (...args: any[]) => logger.error(prefix, ...args),
    debug: (...args: any[]) => logger.debug(prefix, ...args),
  };
};

const shim: ElectronAPI & { __isTauriShim?: boolean } = {
  platform: detectPlatform(),
  onGlobalShortcut: (callback) => attachListener("global-shortcut-event", callback),
  onKeyEvent: (callback) => attachListener("key-event", callback),
  onForceStopMediaRecorder: (callback) =>
    attachListener("force-stop-mediarecorder", callback),
  sendAudioChunk: async (chunk, isFinalChunk) => {
    const payload = Array.from(chunk ?? []);
    await safeInvoke("audio_data_chunk", { chunk: payload, isFinalChunk });
  },
  on: (channel, callback) => {
    attachListener(channel, callback);
  },
  off: (channel, callback) => {
    detachListener(channel, callback);
  },
  log: {
    info: (...args) => baseConsole().info(...args),
    warn: (...args) => baseConsole().warn(...args),
    error: (...args) => baseConsole().error(...args),
    debug: (...args) => baseConsole().debug(...args),
    scope: (name: string) => createScopedLogger(name),
  },
  openExternal: async (url) => {
    if (!url) return;
    if (isTauriRuntime()) {
      try {
        const { openUrl } = await import("@tauri-apps/plugin-opener");
        await openUrl(url);
        return;
      } catch (error) {
        console.warn("[tauri] openExternal failed", error);
      }
    }
    window.open(url, "_blank", "noopener,noreferrer");
  },
  notes: {
    saveYjsUpdate: async (noteId, update) => {
      const bytes = update ? Array.from(new Uint8Array(update)) : [];
      await safeInvoke("notes_save_yjs_update", { noteId, update: bytes });
    },
    loadYjsUpdates: async (noteId) => {
      const result = await safeInvoke<number[][]>("notes_load_yjs_updates", {
        noteId,
      });
      if (!result) return [];
      return result.map((bytes) => new Uint8Array(bytes).buffer);
    },
    replaceYjsUpdates: async (noteId, update) => {
      const bytes = update ? Array.from(new Uint8Array(update)) : [];
      await safeInvoke("notes_replace_yjs_updates", { noteId, update: bytes });
    },
  },
};

shim.__isTauriShim = true;

if (!window.electronAPI) {
  window.electronAPI = shim;
}
