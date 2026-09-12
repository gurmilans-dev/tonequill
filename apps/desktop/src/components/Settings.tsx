import type { DeviceInfo } from '../domain/generated';
import type { Preferences } from '../storage';
import { Icon } from './Icon';

export function DevicePicker({
  devices,
  preferences,
  update,
  disabled = false,
}: {
  devices: DeviceInfo[];
  preferences: Preferences;
  update: (p: Preferences) => void;
  disabled?: boolean;
}) {
  return (
    <div className="device-pickers">
      {([true, false] as const).map((input) => {
        const key = input ? 'input_device' : 'output_device';
        const selected = preferences.devices[key];
        return (
          <label className="field" key={key}>
            <span>
              <Icon name={input ? 'mic' : 'speaker'} size={16} />
              {input ? 'Microphone' : 'Speakers'}
            </span>
            <select
              disabled={disabled}
              value={selected ?? ''}
              onChange={(e) =>
                update({
                  ...preferences,
                  devices: { ...preferences.devices, [key]: e.target.value || null },
                })
              }
            >
              <option value="">System default</option>
              {selected && !devices.some((d) => d.id === selected && d.is_input === input) && (
                <option value={selected}>Selected device unavailable</option>
              )}
              {devices
                .filter((d) => d.is_input === input)
                .map((d) => (
                  <option key={d.id} value={d.id} disabled={!d.compatible}>
                    {d.name}
                    {d.is_default ? ' · default' : ''}
                    {!d.compatible ? ' · unsupported' : ''}
                  </option>
                ))}
            </select>
          </label>
        );
      })}
    </div>
  );
}
export function Settings({
  devices,
  preferences,
  update,
  refresh,
  refreshing,
  locked,
  clearHistory,
  quit,
}: {
  devices: DeviceInfo[];
  preferences: Preferences;
  update: (p: Preferences) => void;
  refresh: () => void;
  refreshing: boolean;
  locked: boolean;
  clearHistory: () => void;
  quit: () => void;
}) {
  return (
    <>
      <div className="page-title">
        <div>
          <p className="eyebrow">MAKE IT YOURS</p>
          <h1>Settings</h1>
          <p className="muted">Everything stays on this computer.</p>
        </div>
      </div>
      <section className="panel settings-panel">
        <div className="section-heading">
          <h2>Audio devices</h2>
          <button className="text-button" onClick={refresh} disabled={refreshing || locked}>
            <Icon name="refresh" size={17} />
            {refreshing ? 'Refreshing…' : 'Refresh devices'}
          </button>
        </div>
        <p className="muted small">
          48 kHz audio is required. Both devices are opened by the shared audio backend.
        </p>
        <DevicePicker
          devices={devices}
          preferences={preferences}
          update={update}
          disabled={locked}
        />
        <details className="technical">
          <summary>Channel & compatibility details</summary>
          <label className="field compact">
            <span>Microphone channel (starts at 0)</span>
            <input
              type="number"
              min={0}
              max={31}
              value={preferences.devices.input_channel}
              disabled={locked}
              onChange={(e) =>
                update({
                  ...preferences,
                  devices: {
                    ...preferences.devices,
                    input_channel: Math.max(0, Math.min(31, Number(e.target.value))),
                  },
                })
              }
            />
          </label>
          <ul className="device-details">
            {devices.map((d) => (
              <li key={`${d.is_input}-${d.id}`}>
                <strong>{d.name}</strong>
                <span>
                  {d.is_input ? 'Input' : 'Output'} ·{' '}
                  {d.compatible ? `48 kHz available · ${d.channels} channels` : d.detail}
                </span>
                <code>{d.id}</code>
              </li>
            ))}
          </ul>
        </details>
      </section>
      <section className="panel settings-panel">
        <h2>Preferences</h2>
        <div className="setting-row">
          <div>
            <strong>Appearance</strong>
            <p>Choose a theme or follow your system.</p>
          </div>
          <select
            aria-label="Appearance"
            value={preferences.theme}
            onChange={(e) =>
              update({ ...preferences, theme: e.target.value as Preferences['theme'] })
            }
          >
            <option value="system">System</option>
            <option value="light">Light</option>
            <option value="dark">Dark</option>
          </select>
        </div>
        <div className="setting-row">
          <div>
            <strong>Maximum session duration</strong>
            <p>A hard limit on audio use and optional WAV recording.</p>
          </div>
          <select
            aria-label="Maximum session duration"
            disabled={locked}
            value={preferences.maxSeconds}
            onChange={(e) => update({ ...preferences, maxSeconds: Number(e.target.value) })}
          >
            {[60, 120, 180, 300, 600, 1800, 3600].map((n) => (
              <option key={n} value={n}>
                {n / 60} minutes
              </option>
            ))}
          </select>
        </div>
        <div className="setting-row">
          <div>
            <strong>Keep recent transfers</strong>
            <p>Store up to 30 completed transfers locally, including file names and paths.</p>
          </div>
          <input
            aria-label="Keep recent transfers"
            type="checkbox"
            checked={preferences.history}
            onChange={(e) => update({ ...preferences, history: e.target.checked })}
          />
        </div>
        <button className="text-button danger-text" onClick={clearHistory}>
          Clear local history
        </button>
      </section>
      <section className="about">
        <Icon name="sound" size={28} />
        <div>
          <strong>
            Tonequill Desktop <span className="muted">0.1.0</span>
          </strong>
          <p>Sound carries the data. Rust handles the integrity.</p>
          <p className="small muted">
            Offline · No accounts · No telemetry
            <br />
            Reliable duplex is experimental. Desktop hardware validation is pending.
          </p>
          <button className="text-button" onClick={quit}>
            Quit Tonequill
          </button>
        </div>
      </section>
    </>
  );
}
