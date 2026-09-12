import { invoke } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open, save } from '@tauri-apps/plugin-dialog';
import type { DeviceInfo, FileInfo, SessionRequest, SessionUpdate } from './domain/generated';

export interface Bridge {
  demo: boolean;
  devices(): Promise<DeviceInfo[]>;
  inspect(path: string): Promise<FileInfo>;
  start(request: SessionRequest): Promise<string>;
  cancel(id: string): Promise<void>;
  poll(after: number): Promise<SessionUpdate>;
  pickFile(): Promise<string | null>;
  pickDestination(suggested: string): Promise<string | null>;
  reveal(path: string): Promise<void>;
  quit(): Promise<void>;
  onDrop(callback: (paths: string[]) => void): Promise<() => void>;
}
export async function createBridge(): Promise<Bridge> {
  if (import.meta.env.DEV && new URLSearchParams(location.search).has('demo')) {
    return (await import('./demo')).createDemo();
  }
  return {
    demo: false,
    devices: () => invoke('audio_devices'),
    inspect: (path) => invoke('inspect_file', { path }),
    start: (request) => invoke('start_session', { request }),
    cancel: (id) => invoke('cancel_session', { id }),
    poll: (after) => invoke('poll_session', { after }),
    pickFile: async () => {
      const path = await open({
        title: 'Choose a file to send',
        multiple: false,
        directory: false,
      });
      return typeof path === 'string' ? path : null;
    },
    pickDestination: (suggested) =>
      save({ title: 'Choose an exact destination', defaultPath: suggested }),
    reveal: (path) => invoke('show_in_folder', { path }),
    quit: () => getCurrentWindow().close(),
    onDrop: async (callback) =>
      getCurrentWebview().onDragDropEvent((event) => {
        if (event.payload.type === 'drop') callback(event.payload.paths);
      }),
  };
}
