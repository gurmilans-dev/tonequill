export type IconName =
  | 'send'
  | 'receive'
  | 'sound'
  | 'arrow'
  | 'file'
  | 'folder'
  | 'check'
  | 'close'
  | 'settings'
  | 'activity'
  | 'clock'
  | 'refresh'
  | 'mic'
  | 'speaker'
  | 'chevron'
  | 'shield'
  | 'warning';
const paths: Record<IconName, string> = {
  send: 'M12 19V5m-6 6 6-6 6 6',
  receive: 'M12 5v14m-6-6 6 6 6-6',
  sound: 'M8 4a10 10 0 0 0 0 16M5 9a4 4 0 0 0 0 6M16 4a10 10 0 0 1 0 16M19 9a4 4 0 0 1 0 6M12 8v8',
  arrow: 'M4 12h16m-6-6 6 6-6 6',
  file: 'M14 3H5v18h14V8l-5-5v5h5M8 13h8M8 17h5',
  folder: 'M3 6h7l2 2h9v12H3z',
  check: 'm5 12 4 4L19 6',
  close: 'm6 6 12 12M6 18 18 6',
  settings: 'M4 7h16M4 17h16M8 4v6M16 14v6',
  activity: 'M2 12h4l3-8 6 16 3-8h4',
  clock: 'M12 8v5l3 2M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0',
  refresh: 'M20 8a9 9 0 1 0 1 8M20 3v6h-6',
  mic: 'M9 4a3 3 0 0 1 6 0v8a3 3 0 0 1-6 0zM6 10v2a6 6 0 0 0 12 0v-2M12 18v4M9 22h6',
  speaker: 'M3 9h4l5-5v16l-5-5H3zM16 8a6 6 0 0 1 0 8M19 5a10 10 0 0 1 0 14',
  chevron: 'm9 5 7 7-7 7',
  shield: 'm12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6zM8 12l3 3 5-6',
  warning: 'm12 3 10 18H2zM12 9v5M12 17v.1',
};
export function Icon({ name, size = 20 }: { name: IconName; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={paths[name]} />
    </svg>
  );
}
