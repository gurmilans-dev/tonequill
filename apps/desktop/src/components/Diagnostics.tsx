import { memo, useState } from 'react';
import type { SessionEvent, SessionSnapshot } from '../domain/generated';
import { duration, eventLabel, phaseLabels } from '../domain/state';

function Value({ label, value }: { label: string; value: string | number | null | undefined }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value ?? '—'}</dd>
    </div>
  );
}
export const Diagnostics = memo(function Diagnostics({
  snapshot: s,
  events,
  truncated,
}: {
  snapshot: SessionSnapshot | null;
  events: SessionEvent[];
  truncated: boolean;
}) {
  const [includeAudio, setIncludeAudio] = useState(false);
  const p = s?.progress;
  const signal = s?.signal;
  const frame = s?.frame;
  const visible = includeAudio ? events : events.filter((e) => e.event.type !== 'signal');
  return (
    <>
      <div className="page-title">
        <div>
          <p className="eyebrow">ENGINEERING VIEW</p>
          <h1>Diagnostics</h1>
          <p className="muted">Measured audio and transfer events from the current session.</p>
        </div>
        <span className="tag">48 kHz · BFSK</span>
      </div>
      {!s && (
        <div className="notice">
          Start a transfer to see live measurements. The microphone is currently closed.
        </div>
      )}
      <div className="diagnostic-grid">
        <section className="panel">
          <h2>Session</h2>
          <dl className="metrics">
            <Value
              label="State"
              value={s?.status.kind === 'active' ? phaseLabels[s.status.phase] : s?.status.kind}
            />
            <Value
              label="Mode"
              value={
                s
                  ? `${s.direction} · ${s.mode === 'one_way' ? 'one-way' : 'reliable (experimental)'}`
                  : null
              }
            />
            <Value label="Elapsed" value={s ? duration(s.elapsed_ms) : null} />
            <Value label="Transfer ID" value={p?.transfer_id} />
            <Value label="Input channel" value={s?.input_channel} />
            <Value label="File" value={s?.file?.name} />
          </dl>
        </section>
        <section className="panel">
          <h2>Packets & frames</h2>
          <dl className="metrics">
            <Value
              label="Data received / confirmed"
              value={p ? `${p.received_packets} / ${p.total_packets ?? 'unknown'}` : null}
            />
            <Value label="Data queued for playback" value={p?.queued_packets} />
            <Value label="CRC-valid frames" value={p?.valid_frames} />
            <Value label="Rejected candidates" value={p?.rejected_frames} />
            <Value label="CRC failures" value={p?.crc_failures} />
            <Value
              label="Duplicates / retransmissions"
              value={p ? `${p.duplicate_frames} / ${p.retransmissions}` : null}
            />
          </dl>
        </section>
        <section className="panel">
          <h2>Microphone</h2>
          <dl className="metrics">
            <Value label="RMS" value={signal?.rms.toFixed(6)} />
            <Value label="Peak" value={signal?.peak.toFixed(6)} />
            <Value label="Captured samples" value={signal?.samples.toLocaleString()} />
            <Value label="Clipped samples" value={signal?.clipped_samples} />
            <Value label="Discontinuities" value={signal?.gaps} />
            <Value label="WAV capture" value={s?.capture_path ?? 'Off'} />
          </dl>
        </section>
        <section className="panel">
          <h2>Last frame acquisition</h2>
          <dl className="metrics">
            <Value label="Acquisition quality" value={frame?.acquisition_quality.toFixed(5)} />
            <Value label="Tone concentration" value={frame?.tone_concentration.toFixed(5)} />
            <Value
              label="1200 / 2200 Hz powers"
              value={
                frame
                  ? `${frame.carrier_powers[0].toExponential(3)} / ${frame.carrier_powers[1].toExponential(3)}`
                  : null
              }
            />
            <Value label="Decision ratio" value={frame?.decision_ratio.toFixed(5)} />
            <Value label="Samples / symbol" value={frame?.samples_per_symbol.toFixed(4)} />
            <Value label="Clock estimate (ppm)" value={frame?.clock_ppm?.toFixed(2)} />
          </dl>
        </section>
      </div>
      {s && (
        <details className="technical">
          <summary>Device IDs, integrity & diagnostic paths</summary>
          <dl className="metrics">
            <Value label="Input selector" value={s.input_device ?? 'System default'} />
            <Value label="Output selector" value={s.output_device ?? 'System default'} />
            <Value label="Expected SHA-256" value={s.file?.sha256} />
            <Value label="Exact destination" value={s.destination} />
            <Value label="Event CSV" value={s.events_path ?? 'Off'} />
          </dl>
        </details>
      )}
      <section className="panel event-panel">
        <div className="section-heading">
          <h2>
            Event log <span className="muted small">{events.length} / 256</span>
          </h2>
          <label className="check-label">
            <input
              type="checkbox"
              checked={includeAudio}
              onChange={(e) => setIncludeAudio(e.target.checked)}
            />
            Include audio levels
          </label>
        </div>
        <p className="small muted">
          {truncated ? 'Older events have been discarded to keep memory bounded. ' : ''}Newest
          first. Timestamps are relative to session start.
        </p>
        <ol className="event-log" aria-label="Session events" tabIndex={0}>
          {[...visible].reverse().map((e) => (
            <li key={e.sequence}>
              <time>
                {duration(e.elapsed_ms)}.{(e.elapsed_ms % 1000).toString().padStart(3, '0')}
              </time>
              <span>{eventLabel(e)}</span>
            </li>
          ))}
        </ol>
        {!events.length && <p className="empty-line">No session events yet.</p>}
      </section>
    </>
  );
});
