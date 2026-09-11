/// <reference types="vite/client" />
/// <reference types="unplugin-icons/types/react" />

export {};

declare global {
  interface Window {
    ipc?: { postMessage: (msg: string) => void };
    __AIUB_APPLY__?: (raw: unknown) => void;
    __AIUB_LOCKCLICKS__?: (ms: number) => void;
    __AIUB_VISIBLE__?: (visible: boolean) => void;
  }
}
