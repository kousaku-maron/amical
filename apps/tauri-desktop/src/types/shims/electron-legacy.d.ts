declare module "electron" {
  export const app: any;
  export const ipcMain: any;
  export const ipcRenderer: any;
  export const shell: any;
  export const dialog: any;
  export const Tray: any;
  export const Menu: any;
  export const BrowserWindow: any;
  export const nativeTheme: any;
  export const nativeImage: any;
  export const globalShortcut: any;
  export const systemPreferences: any;
  export const contextBridge: any;
  export type IpcRendererEvent = any;
}

declare module "electron-log" {
  const log: any;
  export default log;
}

declare module "electron-squirrel-startup" {
  const started: any;
  export default started;
}

declare module "update-electron-app" {
  export const updateElectronApp: (...args: any[]) => any;
}

declare module "electron-trpc-experimental/*" {
  export const createIPCHandler: any;
  export const exposeElectronTRPC: any;
  export const ipcLink: any;
}
