import type {
  Completion,
  DeviceSelection,
  Direction,
  TransferMode,
  SessionSnapshot,
} from './domain/generated';

export interface Preferences {
  theme: 'system' | 'light' | 'dark';
  devices: DeviceSelection;
  maxSeconds: number;
  history: boolean;
}
export const defaultPreferences: Preferences = {
  theme: 'system',
  devices: { input_device: null, output_device: null, input_channel: 0 },
  maxSeconds: 180,
  history: true,
};
export interface Recent {
  id: string;
  date: string;
  direction: Direction;
  mode: TransferMode;
  elapsed_ms: number;
  result: Completion;
}
const KEY = 'tonequill.preferences.v1';
const RECENT = 'tonequill.recent.v1';
export function readPreferences(): Preferences {
  const raw: unknown = JSON.parse(localStorage.getItem(KEY) ?? 'null');
  if (!raw || typeof raw !== 'object') return defaultPreferences;
  const p = raw as Partial<Preferences>;
  const d = p.devices;
  return {
    theme: p.theme === 'light' || p.theme === 'dark' ? p.theme : 'system',
    devices: {
      input_device: typeof d?.input_device === 'string' ? d.input_device : null,
      output_device: typeof d?.output_device === 'string' ? d.output_device : null,
      input_channel:
        typeof d?.input_channel === 'number' &&
        Number.isInteger(d.input_channel) &&
        d.input_channel >= 0 &&
        d.input_channel < 32
          ? d.input_channel
          : 0,
    },
    maxSeconds:
      typeof p.maxSeconds === 'number' && p.maxSeconds >= 60 && p.maxSeconds <= 3600
        ? Math.floor(p.maxSeconds)
        : 180,
    history: p.history !== false,
  };
}
export function savePreferences(p: Preferences) {
  localStorage.setItem(KEY, JSON.stringify(p));
}
export function readRecent(): Recent[] {
  const text = localStorage.getItem(RECENT);
  if (!text || text.length > 200_000) return [];
  const value: unknown = JSON.parse(text);
  if (!Array.isArray(value)) return [];
  // History is display-only. Validate persisted data before it reaches rendering.
  return value
    .filter((item: unknown): item is Recent => {
      if (!item || typeof item !== 'object') return false;
      const x = item as Partial<Recent>;
      const r = x.result;
      return (
        typeof x.id === 'string' &&
        typeof x.date === 'string' &&
        Number.isFinite(Date.parse(x.date)) &&
        (x.direction === 'send' || x.direction === 'receive') &&
        (x.mode === 'one_way' || x.mode === 'reliable') &&
        typeof x.elapsed_ms === 'number' &&
        !!r &&
        (r.kind === 'received'
          ? typeof r.output_path === 'string'
          : r.kind === 'sent' && typeof r.peer_verified === 'boolean') &&
        typeof r.file?.name === 'string' &&
        typeof r.file.bytes === 'number' &&
        typeof r.file.sha256 === 'string'
      );
    })
    .slice(0, 30);
}
export function addRecent(existing: Recent[], snapshot: SessionSnapshot): Recent[] {
  if (snapshot.status.kind !== 'completed' || existing.some((x) => x.id === snapshot.id))
    return existing;
  return [
    {
      id: snapshot.id,
      date: new Date().toISOString(),
      direction: snapshot.direction,
      mode: snapshot.mode,
      elapsed_ms: snapshot.elapsed_ms,
      result: snapshot.status.result,
    },
    ...existing,
  ].slice(0, 30);
}
export function saveRecent(items: Recent[]) {
  localStorage.setItem(RECENT, JSON.stringify(items.slice(0, 30)));
}
