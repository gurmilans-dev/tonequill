import { useCallback, useEffect, useReducer, useRef, useState } from 'react';
import type { Bridge } from './bridge';
import type { DeviceInfo, FileInfo, TransferMode } from './domain/generated';
import {
  asFailure,
  basename,
  bytes,
  duration,
  failureCopy,
  initialSession,
  sessionReducer,
} from './domain/state';
import {
  addRecent,
  defaultPreferences,
  readPreferences,
  readRecent,
  savePreferences,
  saveRecent,
} from './storage';
import type { Preferences, Recent } from './storage';
import { Icon } from './components/Icon';
import type { IconName } from './components/Icon';
import { Diagnostics } from './components/Diagnostics';
import { SessionPanel } from './components/SessionPanel';
import { DevicePicker, Settings } from './components/Settings';

type View = 'home' | 'send' | 'receive' | 'diagnostics' | 'settings' | 'recent';
function load<T>(reader: () => T, fallback: T): { value: T; error: string | null } {
  try {
    return { value: reader(), error: null };
  } catch {
    return {
      value: fallback,
      error: 'Local preferences or history could not be read. Defaults are in use.',
    };
  }
}
export function App({ bridge }: { bridge: Bridge }) {
  const [initial] = useState(() => ({
    prefs: load(readPreferences, defaultPreferences),
    recent: load(readRecent, []),
  }));
  const [preferences, setPreferences] = useState(initial.prefs.value);
  const [recent, setRecent] = useState<Recent[]>(initial.recent.value);
  const [notice, setNotice] = useState<string | null>(initial.prefs.error ?? initial.recent.error);
  const [view, setView] = useState<View>('home');
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [refreshing, setRefreshing] = useState(false);
  const [source, setSource] = useState<{ path: string; info: FileInfo } | null>(null);
  const [inspecting, setInspecting] = useState(false);
  const [destination, setDestination] = useState('');
  const [mode, setMode] = useState<TransferMode>('one_way');
  const [overwrite, setOverwrite] = useState(false);
  const [capture, setCapture] = useState<string | null>(null);
  const [eventPath, setEventPath] = useState<string | null>(null);
  const [session, dispatch] = useReducer(sessionReducer, initialSession);
  const latest = useRef(session);
  const after = useRef(0);
  const inspectionId = useRef(0);
  const heading = useRef<HTMLElement>(null);
  const locked = session.pending !== null || session.snapshot?.status.kind === 'active';
  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      setDevices(await bridge.devices());
    } catch (e) {
      dispatch({ type: 'error', error: asFailure(e) });
    } finally {
      setRefreshing(false);
    }
  }, [bridge]);
  useEffect(() => {
    latest.current = session;
  }, [session]);
  useEffect(() => {
    void refresh();
  }, [refresh]);
  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const update = await bridge.poll(after.current);
        if (stopped) return;
        if (
          update.snapshot &&
          latest.current.pending !== 'start' &&
          (!latest.current.expectedId || update.snapshot.id === latest.current.expectedId)
        )
          after.current = update.snapshot.last_sequence;
        dispatch({ type: 'update', update });
      } catch (e) {
        if (!stopped) setNotice(`Cannot read session status: ${asFailure(e).detail}`);
      }
      if (!stopped)
        timer = setTimeout(() => {
          void poll();
        }, 250);
    };
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [bridge]);
  useEffect(() => {
    const media = matchMedia('(prefers-color-scheme: dark)');
    const apply = () => {
      document.documentElement.dataset.theme =
        preferences.theme === 'system' ? (media.matches ? 'dark' : 'light') : preferences.theme;
    };
    apply();
    media.addEventListener('change', apply);
    return () => media.removeEventListener('change', apply);
  }, [preferences.theme]);
  const updatePreferences = (next: Preferences) => {
    setPreferences(next);
    try {
      savePreferences(next);
    } catch {
      setNotice('Preferences could not be saved locally. Current selections still apply.');
    }
  };
  useEffect(() => {
    const s = session.snapshot;
    if (!s || s.status.kind !== 'completed' || !preferences.history || bridge.demo) return;
    setRecent((existing) => {
      const next = addRecent(existing, s);
      if (next !== existing) {
        try {
          saveRecent(next);
        } catch {
          setNotice('Transfer completed, but local history could not be saved.');
        }
      }
      return next;
    });
  }, [session.snapshot, preferences.history, bridge.demo]);
  useEffect(() => {
    heading.current?.focus({ preventScroll: true });
  }, [view]);
  const chooseSource = useCallback(
    async (path: string) => {
      const id = ++inspectionId.current;
      setInspecting(true);
      setSource(null);
      try {
        const info = await bridge.inspect(path);
        if (id === inspectionId.current) {
          setSource({ path, info });
          setView('send');
        }
      } catch (e) {
        if (id === inspectionId.current) dispatch({ type: 'error', error: asFailure(e) });
      } finally {
        if (id === inspectionId.current) setInspecting(false);
      }
    },
    [bridge],
  );
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    bridge
      .onDrop((paths) => {
        if (locked) return;
        if (paths.length !== 1 || !paths[0]) {
          setNotice('Drop one file at a time.');
          return;
        }
        dispatch({ type: 'reset' });
        void chooseSource(paths[0]);
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((e) => setNotice(`File drop is unavailable: ${asFailure(e).detail}`));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [bridge, locked, chooseSource]);
  const pickSource = async () => {
    try {
      const path = await bridge.pickFile();
      if (path) await chooseSource(path);
    } catch (e) {
      dispatch({ type: 'error', error: asFailure(e) });
    }
  };
  const pickDestination = async () => {
    try {
      const path = await bridge.pickDestination(
        `received-${new Date().toISOString().replace(/[:.]/g, '-')}.bin`,
      );
      if (path) {
        setDestination(path);
        setOverwrite(false);
      }
    } catch (e) {
      dispatch({ type: 'error', error: asFailure(e) });
    }
  };
  const diagnosticPath = async (kind: 'capture' | 'events') => {
    try {
      const path = await bridge.pickDestination(
        `tonequill-${Date.now()}.${kind === 'capture' ? 'wav' : 'csv'}`,
      );
      if (path) {
        if (kind === 'capture') setCapture(path);
        else setEventPath(path);
      }
    } catch (e) {
      dispatch({ type: 'error', error: asFailure(e) });
    }
  };
  const start = async () => {
    if (locked || (view !== 'send' && view !== 'receive')) return;
    dispatch({ type: 'starting' });
    after.current = 0;
    try {
      const id = await bridge.start({
        direction: view,
        mode,
        path: view === 'send' ? (source?.path ?? '') : destination,
        devices: preferences.devices,
        capture_path: view === 'receive' ? capture : null,
        events_path: eventPath,
        max_seconds: preferences.maxSeconds,
        idle_timeout_seconds: Math.min(120, preferences.maxSeconds),
        overwrite: view === 'receive' && overwrite,
        expected_sha256: view === 'send' ? (source?.info.sha256 ?? null) : null,
      });
      after.current = 0;
      dispatch({ type: 'started', id });
    } catch (e) {
      dispatch({ type: 'error', error: asFailure(e) });
    }
  };
  const cancel = async () => {
    const s = session.snapshot;
    if (!s || session.pending) return;
    dispatch({ type: 'cancelling' });
    try {
      await bridge.cancel(s.id);
      dispatch({ type: 'update', update: await bridge.poll(after.current) });
    } catch (e) {
      dispatch({ type: 'error', error: asFailure(e) });
    }
  };
  const reveal = async (path: string) => {
    try {
      await bridge.reveal(path);
    } catch (e) {
      dispatch({ type: 'error', error: asFailure(e) });
    }
  };
  const newTransfer = (next: 'send' | 'receive') => {
    if (locked) {
      setView(session.snapshot?.direction ?? next);
      return;
    }
    dispatch({ type: 'reset' });
    setView(next);
    setCapture(null);
    setEventPath(null);
    setOverwrite(false);
    if (next === 'receive') setDestination('');
  };
  const clearHistory = () => {
    try {
      saveRecent([]);
      setRecent([]);
    } catch {
      setNotice('Local history could not be cleared.');
    }
  };
  const selectedDevice = (input: boolean) => {
    const id = input ? preferences.devices.input_device : preferences.devices.output_device;
    return devices.find((d) => d.is_input === input && (id ? d.id === id : d.is_default));
  };
  const input = selectedDevice(true);
  const output = selectedDevice(false);
  const ready = !!input?.compatible && !!output?.compatible;
  const canStart =
    ready &&
    !locked &&
    !inspecting &&
    (view === 'receive'
      ? !!destination
      : !!source && source.info.duration_seconds < preferences.maxSeconds);
  const showSession =
    session.snapshot && (view === 'home' || view === 'send' || view === 'receive');
  const nav: { view: View; label: string; icon: IconName }[] = [
    { view: 'home', label: 'Transfer', icon: 'sound' },
    { view: 'recent', label: 'Recent', icon: 'clock' },
    { view: 'diagnostics', label: 'Diagnostics', icon: 'activity' },
    { view: 'settings', label: 'Settings', icon: 'settings' },
  ];
  return (
    <div className="app-shell">
      <aside className="sidebar">
        <a
          className="brand"
          href="#main"
          onClick={(e) => {
            e.preventDefault();
            setView('home');
          }}
        >
          <span className="brand-symbol">
            <Icon name="sound" size={25} />
          </span>
          Tonequill<span className="brand-dot">●</span>
        </a>
        <p className="sidebar-label">YOUR WORKSPACE</p>
        <nav aria-label="Main navigation">
          {nav.map((n) => (
            <button
              key={n.view}
              className={`nav-item ${view === n.view || (n.view === 'home' && (view === 'send' || view === 'receive')) ? 'selected' : ''}`}
              aria-current={
                view === n.view || (n.view === 'home' && (view === 'send' || view === 'receive'))
                  ? 'page'
                  : undefined
              }
              onClick={() => setView(n.view)}
            >
              <Icon name={n.icon} />
              {n.label}
              {n.view === 'home' && locked && <span className="live-dot" />}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <span className="offline-dot" /> Local by design<p>No network. Just sound.</p>
          <span className="version">DESKTOP / 0.1</span>
        </div>
      </aside>
      <div className="workspace">
        <header className="topbar">
          <span>ACOUSTIC FILE TRANSFER</span>
          <div>
            <span className={`status-dot ${locked ? 'active' : ''}`} />
            {locked ? 'Audio session active' : 'Audio devices idle'}
          </div>
        </header>
        <main id="main" ref={heading} tabIndex={-1}>
          {bridge.demo && (
            <div className="notice demo-banner">
              DEVELOPMENT DEMO · Simulated events · No audio or files are used
            </div>
          )}
          {notice && (
            <div className="notice warning" role="alert">
              <span>{notice}</span>
              <button
                className="icon-button"
                aria-label="Dismiss notice"
                onClick={() => setNotice(null)}
              >
                <Icon name="close" size={16} />
              </button>
            </div>
          )}
          {session.error && (
            <div className="error-banner" role="alert">
              <strong>{failureCopy[session.error.code]?.[0] ?? 'Unable to continue'}</strong>
              <p>{failureCopy[session.error.code]?.[1]}</p>
              <details>
                <summary>Technical details</summary>
                <p>{session.error.detail}</p>
              </details>
            </div>
          )}
          {showSession ? (
            <SessionPanel
              s={session.snapshot!}
              cancelling={session.pending === 'cancel'}
              cancel={() => void cancel()}
              again={() => newTransfer(session.snapshot!.direction)}
              diagnostics={() => setView('diagnostics')}
              reveal={(p) => void reveal(p)}
            />
          ) : view === 'home' ? (
            <>
              <div className="home-heading">
                <p className="eyebrow">A DIFFERENT WAY TO CONNECT</p>
                <h1>
                  Let sound carry
                  <br />
                  your files.
                </h1>
                <p>
                  Move small files between nearby devices.
                  <br />
                  Through speakers, air and a microphone.
                </p>
              </div>
              <div className="action-grid">
                <button className="action-card send-card" onClick={() => newTransfer('send')}>
                  <span className="action-icon">
                    <Icon name="send" size={27} />
                  </span>
                  <span className="action-index">01 / OUTGOING</span>
                  <strong>Send a file</strong>
                  <span>Choose a file. Turn it into sound.</span>
                  <span className="action-bottom">
                    Start a transmission
                    <Icon name="arrow" size={23} />
                  </span>
                </button>
                <button className="action-card receive-card" onClick={() => newTransfer('receive')}>
                  <span className="action-icon">
                    <Icon name="receive" size={27} />
                  </span>
                  <span className="action-index">02 / INCOMING</span>
                  <strong>Receive a file</strong>
                  <span>Listen for a nearby transmission.</span>
                  <span className="action-bottom">
                    Open your microphone
                    <Icon name="arrow" size={23} />
                  </span>
                </button>
              </div>
              <div className="home-note">
                <Icon name="shield" size={19} />
                <p>
                  Every received file is checked, packet by packet.
                  <br />
                  <strong>Saved only after SHA-256 verification.</strong>
                </p>
              </div>
              <section className="home-devices">
                <div className="section-heading">
                  <h2>Your audio setup</h2>
                  <button className="text-button" onClick={() => setView('settings')}>
                    Manage
                    <Icon name="chevron" size={16} />
                  </button>
                </div>
                <div className="device-summary">
                  <div>
                    <Icon name="mic" />
                    <span>
                      <small>MICROPHONE</small>
                      {input?.name ?? 'No compatible default input'}
                    </span>
                  </div>
                  <div>
                    <Icon name="speaker" />
                    <span>
                      <small>SPEAKERS</small>
                      {output?.name ?? 'No compatible default output'}
                    </span>
                  </div>
                </div>
              </section>
            </>
          ) : view === 'send' || view === 'receive' ? (
            <>
              <button className="text-button back" onClick={() => setView('home')}>
                ← All transfers
              </button>
              <div className="page-title">
                <div>
                  <p className="eyebrow">{view === 'send' ? 'OUTGOING' : 'INCOMING'}</p>
                  <h1>{view === 'send' ? 'Send a file.' : 'Ready when you are.'}</h1>
                  <p className="muted">
                    {view === 'send'
                      ? 'Choose a small file and a listening device nearby.'
                      : 'Choose where to save, then start listening.'}
                  </p>
                </div>
                <span className="outline-symbol">
                  <Icon name={view} size={32} />
                </span>
              </div>
              <section className="panel setup-panel">
                <div className="step-label">
                  <span>1</span>
                  <h2>{view === 'send' ? 'Choose your file' : 'Choose a destination'}</h2>
                </div>
                {view === 'send' ? (
                  <>
                    <button
                      className={`drop-zone ${source ? 'has-file' : ''}`}
                      disabled={locked || inspecting}
                      onClick={() => void pickSource()}
                    >
                      <span className="file-icon">
                        <Icon name="file" size={27} />
                      </span>
                      <strong>
                        {inspecting
                          ? 'Inspecting file…'
                          : (source?.info.name ?? 'Drop a file here')}
                      </strong>
                      <span>
                        {source
                          ? `${bytes(source.info.bytes)} · click to choose another file`
                          : 'or click to browse your computer'}
                      </span>
                    </button>
                    {source && (
                      <div className="estimate">
                        <div>
                          <small>DATA PACKETS</small>
                          <strong>{source.info.data_packets}</strong>
                        </div>
                        <div>
                          <small>MINIMUM AUDIO TIME</small>
                          <strong>{source.info.duration_seconds.toFixed(1)} s</strong>
                        </div>
                        <div>
                          <small>TOTAL FRAMES</small>
                          <strong>{source.info.total_frames}</strong>
                        </div>
                      </div>
                    )}
                    <p className="small muted">
                      Optimized for small files. Guard time, feedback and retries add to the
                      estimate.
                    </p>
                    {source && source.info.duration_seconds >= preferences.maxSeconds && (
                      <div className="notice warning">
                        This file exceeds the {preferences.maxSeconds / 60}-minute session limit.
                        Increase the limit in Settings or choose a smaller file.
                      </div>
                    )}
                  </>
                ) : (
                  <>
                    <button
                      className="destination-picker"
                      disabled={locked}
                      onClick={() => void pickDestination()}
                    >
                      <span className="file-icon">
                        <Icon name="folder" size={26} />
                      </span>
                      <span>
                        <strong>
                          {destination ? basename(destination) : 'Choose where to save'}
                        </strong>
                        <small>{destination || 'Select an exact file name on this computer'}</small>
                      </span>
                      <Icon name="chevron" />
                    </button>
                    {destination && (
                      <label className="check-label overwrite">
                        <input
                          type="checkbox"
                          checked={overwrite}
                          onChange={(e) => setOverwrite(e.target.checked)}
                          disabled={locked}
                        />
                        Allow replacing an existing file at this exact destination
                      </label>
                    )}
                    <p className="small muted">
                      The transmitted name is informational. Only your chosen destination can be
                      written.
                    </p>
                  </>
                )}
              </section>
              <section className="panel setup-panel">
                <div className="step-label">
                  <span>2</span>
                  <h2>Set up the link</h2>
                </div>
                <DevicePicker
                  devices={devices}
                  preferences={preferences}
                  update={updatePreferences}
                  disabled={locked}
                />
                <fieldset className="mode-picker" disabled={locked}>
                  <legend>Transfer mode</legend>
                  <label className={mode === 'one_way' ? 'chosen' : ''}>
                    <input
                      type="radio"
                      name="mode"
                      value="one_way"
                      checked={mode === 'one_way'}
                      onChange={() => setMode('one_way')}
                    />
                    <span>
                      <strong>
                        One-way<span className="tiny-tag">DEFAULT</span>
                      </strong>
                      <small>
                        {view === 'receive'
                          ? 'Receive a WAV played from your phone.'
                          : 'Play the file as audio. No return confirmation.'}
                      </small>
                    </span>
                  </label>
                  <label className={mode === 'reliable' ? 'chosen' : ''}>
                    <input
                      type="radio"
                      name="mode"
                      value="reliable"
                      checked={mode === 'reliable'}
                      onChange={() => setMode('reliable')}
                    />
                    <span>
                      <strong>
                        Reliable duplex<span className="tiny-tag amber">EXPERIMENTAL</span>
                      </strong>
                      <small>Two Tonequill computers. Feedback and retries.</small>
                    </span>
                  </label>
                </fieldset>
                {mode === 'reliable' && (
                  <div className="notice warning">
                    Requires Tonequill on both devices, with working microphones and speakers.
                    Physical duplex validation is pending.
                  </div>
                )}
                <details className="technical">
                  <summary>Optional diagnostic recording</summary>
                  <p className="small muted">
                    Off by default. Choose new file names. Recording stops with the session, at most{' '}
                    {preferences.maxSeconds / 60} minutes (microphone WAV up to{' '}
                    {Math.ceil((preferences.maxSeconds * 96000) / 1024 ** 2)} MiB).
                  </p>
                  {view === 'receive' && (
                    <div className="diagnostic-option">
                      <label className="check-label">
                        <input
                          type="checkbox"
                          checked={!!capture}
                          disabled={locked}
                          onChange={(e) => {
                            if (e.target.checked) void diagnosticPath('capture');
                            else setCapture(null);
                          }}
                        />
                        Save microphone WAV
                      </label>
                      {capture && <p>{capture}</p>}
                    </div>
                  )}
                  <div className="diagnostic-option">
                    <label className="check-label">
                      <input
                        type="checkbox"
                        checked={!!eventPath}
                        disabled={locked}
                        onChange={(e) => {
                          if (e.target.checked) void diagnosticPath('events');
                          else setEventPath(null);
                        }}
                      />
                      Save event CSV
                    </label>
                    {eventPath && <p>{eventPath}</p>}
                  </div>
                </details>
              </section>
              <div className="start-row">
                <p>
                  <Icon name="clock" size={16} />
                  {preferences.maxSeconds / 60}-minute limit<span>Change in Settings</span>
                </p>
                <button
                  className="button primary"
                  disabled={!canStart}
                  onClick={() => void start()}
                >
                  <Icon name={view} />
                  {session.pending === 'start'
                    ? 'Preparing…'
                    : view === 'receive'
                      ? 'Start listening'
                      : 'Start transmission'}
                </button>
              </div>
              {!ready && (
                <p className="small warning-text">
                  {refreshing
                    ? 'Checking audio devices…'
                    : 'Select compatible input and output devices to continue. Refresh the device list in Settings if needed.'}
                </p>
              )}
              {view === 'receive' && (
                <div className="listen-tip">
                  <strong>Using your phone?</strong>
                  <p>
                    Start listening first. Then play the original Tonequill WAV from the beginning,
                    with the phone near the PC microphone.
                  </p>
                </div>
              )}
            </>
          ) : null}
          {view === 'diagnostics' && (
            <Diagnostics
              snapshot={session.snapshot}
              events={session.events}
              truncated={session.truncated}
            />
          )}
          {view === 'settings' && (
            <Settings
              devices={devices}
              preferences={preferences}
              update={updatePreferences}
              refresh={() => void refresh()}
              refreshing={refreshing}
              locked={locked}
              clearHistory={clearHistory}
              quit={() => {
                void bridge.quit().catch((e) => dispatch({ type: 'error', error: asFailure(e) }));
              }}
            />
          )}
          {view === 'recent' && (
            <>
              <div className="page-title">
                <div>
                  <p className="eyebrow">ON THIS COMPUTER</p>
                  <h1>Recent transfers</h1>
                  <p className="muted">Up to 30 completed sessions, stored locally.</p>
                </div>
                {recent.length > 0 && (
                  <button className="text-button" onClick={clearHistory}>
                    Clear history
                  </button>
                )}
              </div>
              {recent.length ? (
                <div className="recent-list">
                  {recent.map((r) => (
                    <article key={r.id}>
                      <span className="file-icon">
                        <Icon name={r.direction} />
                      </span>
                      <div>
                        <strong>{r.result.file.name}</strong>
                        <p>
                          {bytes(r.result.file.bytes)} · {new Date(r.date).toLocaleString()} ·{' '}
                          {duration(r.elapsed_ms)}
                        </p>
                        <span className="small muted">
                          {r.result.kind === 'received'
                            ? 'Received · SHA-256 verified'
                            : r.result.peer_verified
                              ? 'Sent · receiver verified'
                              : 'Sent · playback complete'}
                        </span>
                      </div>
                      {r.result.kind === 'received' && (
                        <button
                          className="icon-button"
                          aria-label={`Show ${r.result.file.name} in folder`}
                          onClick={() => {
                            if (r.result.kind === 'received') void reveal(r.result.output_path);
                          }}
                        >
                          <Icon name="folder" />
                        </button>
                      )}
                    </article>
                  ))}
                </div>
              ) : (
                <div className="empty-state">
                  <Icon name="clock" size={38} />
                  <h2>A fresh start.</h2>
                  <p>Completed transfers will appear here.</p>
                  <button className="button secondary" onClick={() => setView('home')}>
                    Make your first transfer
                    <Icon name="arrow" />
                  </button>
                </div>
              )}
            </>
          )}
        </main>
        <footer className="workspace-footer">
          <span>Built for nearby devices.</span>
          <span>CRC32 + SHA-256 integrity</span>
        </footer>
      </div>
    </div>
  );
}
