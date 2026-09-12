import type { SessionSnapshot } from '../domain/generated';
import { bytes, duration, failureCopy, percent, phaseLabels, signalLabel } from '../domain/state';
import { Icon } from './Icon';

export function SessionPanel({
  s,
  cancelling,
  cancel,
  again,
  diagnostics,
  reveal,
}: {
  s: SessionSnapshot;
  cancelling: boolean;
  cancel: () => void;
  again: () => void;
  diagnostics: () => void;
  reveal: (path: string) => void;
}) {
  const status = s.status;
  const progress = s.progress;
  const value = percent(s);
  const receiving = s.direction === 'receive';
  const complete = status.kind === 'completed';
  const received = complete && status.result.kind === 'received';
  const sentVerified = complete && status.result.kind === 'sent' && status.result.peer_verified;
  const title = cancelling
    ? 'Stopping audio…'
    : status.kind === 'active'
      ? phaseLabels[status.phase]
      : complete
        ? received
          ? 'File received.'
          : sentVerified
            ? 'Delivery confirmed.'
            : 'Playback complete.'
        : status.kind === 'cancelled'
          ? 'Session cancelled.'
          : failureCopy[status.error.code][0];
  const subtitle = cancelling
    ? 'Waiting for the audio worker to finish and release your devices.'
    : status.kind === 'active'
      ? receiving
        ? 'Keep the phone steady and play the entire Tonequill WAV.'
        : 'Keep the receiving device listening until playback finishes.'
      : complete
        ? received
          ? 'Every data packet passed CRC checks. The file’s SHA-256 matches.'
          : sentVerified
            ? 'The receiver confirmed a verified, saved file.'
            : 'The audio was sent. One-way mode cannot confirm reception.'
        : status.kind === 'cancelled'
          ? receiving
            ? 'The destination was left untouched. Audio devices have been released.'
            : 'Playback stopped. Audio devices have been released.'
          : failureCopy[status.error.code][1];
  return (
    <section className={`session-card ${complete ? 'success' : ''}`}>
      <div className="section-heading">
        <span className="eyebrow">{receiving ? 'RECEIVE' : 'SEND'} SESSION</span>
        <span className="tag">{s.mode === 'one_way' ? 'One-way' : 'Reliable · experimental'}</span>
      </div>
      <div
        className={`status-symbol ${complete ? 'done' : status.kind === 'failed' ? 'failed' : ''}`}
      >
        <Icon
          name={
            complete
              ? 'check'
              : status.kind === 'failed'
                ? 'warning'
                : status.kind === 'cancelled'
                  ? 'close'
                  : receiving
                    ? 'receive'
                    : 'send'
          }
          size={34}
        />
      </div>
      <div role="status" aria-live="polite">
        <h1>{title}</h1>
        <p className="session-subtitle">{subtitle}</p>
      </div>
      <div className="file-row">
        <span className="file-icon">
          <Icon name="file" size={25} />
        </span>
        <div>
          <strong>{s.file?.name ?? 'Waiting for file information'}</strong>
          <span>
            {s.file
              ? `${bytes(s.file.bytes)} · ${s.file.data_packets} data packet${s.file.data_packets === 1 ? '' : 's'}`
              : 'Total size and packet count are not known yet.'}
          </span>
        </div>
        {received && (
          <span className="verified">
            <Icon name="shield" size={16} />
            Verified
          </span>
        )}
      </div>
      <div className="transfer-progress">
        <div className="progress-label">
          <span>
            {!receiving && s.mode === 'one_way' ? 'Data packets queued' : 'Data packets'}{' '}
            <strong>
              {!receiving && s.mode === 'one_way'
                ? progress.queued_packets
                : progress.received_packets}{' '}
              / {progress.total_packets ?? '—'}
            </strong>
          </span>
          <span className="mono">{duration(s.elapsed_ms)}</span>
        </div>
        <div
          className={`progress-track ${value === null && status.kind === 'active' ? 'indeterminate' : ''}`}
          role="progressbar"
          aria-label="Data packet progress"
          aria-valuenow={value ?? undefined}
          aria-valuemin={0}
          aria-valuemax={100}
        >
          <span
            style={{
              width: value === null ? (status.kind === 'active' ? undefined : '0%') : `${value}%`,
            }}
          />
        </div>
        {value === 100 && status.kind === 'active' && (
          <p className="small muted">
            {receiving
              ? 'All data packets collected. Finishing verification and session cleanup.'
              : 'Waiting for playback and session cleanup to finish.'}
          </p>
        )}
      </div>
      {status.kind === 'active' && receiving && (
        <div className="signal-strip">
          <Icon name="mic" />
          <span>{signalLabel(s)}</span>
          <meter aria-label="Microphone peak" min={0} max={1} value={s.signal?.peak ?? 0} />
          <span className="small muted">{progress.valid_frames} valid frames</span>
        </div>
      )}
      {s.signal && s.signal.clipped_samples > 0 && (
        <div className="notice warning">
          Clipping was detected. Lower the playback or microphone level for the next attempt.
        </div>
      )}
      {s.signal && s.signal.gaps > 0 && (
        <p className="small muted">
          {s.signal.gaps} audio discontinuity detected. Missing audio may require a full replay.
        </p>
      )}
      {status.kind === 'failed' && (
        <>
          <p className="no-file">
            {receiving
              ? 'File written: No. The destination was left untouched.'
              : 'Delivery was not confirmed.'}
          </p>
          <details className="technical">
            <summary>Technical details</summary>
            <p>{status.error.detail}</p>
          </details>
        </>
      )}
      {complete && (
        <details className="technical">
          <summary>Transfer details</summary>
          <dl className="metrics">
            <div>
              <dt>Elapsed</dt>
              <dd>{(s.elapsed_ms / 1000).toFixed(1)} seconds</dd>
            </div>
            <div>
              <dt>SHA-256</dt>
              <dd>{status.result.file.sha256}</dd>
            </div>
            <div>
              <dt>Duplicates / retransmissions</dt>
              <dd>
                {progress.duplicate_frames} / {progress.retransmissions}
              </dd>
            </div>
            {status.result.kind === 'received' && (
              <div>
                <dt>Saved to</dt>
                <dd>{status.result.output_path}</dd>
              </div>
            )}
          </dl>
        </details>
      )}
      {s.warnings.map((w, i) => (
        <div className="notice warning" key={i}>
          {w.detail}
        </div>
      ))}
      <div className="session-actions">
        {status.kind === 'active' ? (
          <button className="button secondary" onClick={cancel} disabled={cancelling}>
            <Icon name="close" />
            {cancelling ? 'Stopping…' : 'Cancel session'}
          </button>
        ) : (
          <>
            <button className="button primary" onClick={again}>
              {receiving ? 'Receive another' : 'Send another'}
              <Icon name="arrow" />
            </button>
            {status.kind === 'completed' && status.result.kind === 'received' && (
              <button
                className="button secondary"
                onClick={() => {
                  if (status.result.kind === 'received') reveal(status.result.output_path);
                }}
              >
                <Icon name="folder" />
                Show in folder
              </button>
            )}
          </>
        )}
        <button className="text-button" onClick={diagnostics}>
          View diagnostics
          <Icon name="chevron" size={16} />
        </button>
      </div>
    </section>
  );
}
