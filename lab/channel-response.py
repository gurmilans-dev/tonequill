"""Offline, reference-labelled channel measurements; never a decoder.

Input CSVs come from diagnose-frames. Requires numpy. The reference bits are
used to measure powers and linear channel memory, not to recover a packet.
Both in-sample and training-to-payload fits are explicitly labelled.
"""
import argparse
import csv
import hashlib
import json
import wave
from pathlib import Path

import numpy as np


def rms(samples):
    return float(np.sqrt(np.mean(samples**2))) if len(samples) else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("recording", type=Path)
    parser.add_argument("frames_csv", type=Path)
    parser.add_argument("symbols_csv", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    with wave.open(str(args.recording), "rb") as source:
        if (source.getframerate(), source.getnchannels(), source.getsampwidth()) != (48000, 1, 2):
            raise ValueError("expected 48 kHz mono PCM16 WAV")
        samples = np.frombuffer(source.readframes(source.getnframes()), "<i2").astype(float) / 32767
    with args.frames_csv.open(newline="", encoding="utf-8-sig") as source:
        frames = list(csv.DictReader(source))
    with args.symbols_csv.open(newline="", encoding="utf-8-sig") as source:
        rows = list(csv.DictReader(source))
    # Independent sample-resolution correlators; the Rust receiver uses HOP=24.
    tracks = []
    time = np.arange(len(samples)) / 48000
    for hz in [1200, 2200]:
        cumulative = np.r_[0, np.cumsum(samples * np.exp(-2j * np.pi * hz * time))]
        tracks.append((cumulative[240:] - cumulative[:-240]) / 120)
    result = {
        "recording": str(args.recording),
        "sha256": hashlib.sha256(args.recording.read_bytes()).hexdigest(),
        "duration_s": len(samples) / 48000,
        "rms": rms(samples),
        "peak": float(np.max(abs(samples))),
        "pcm_rail_samples": int(np.count_nonzero(abs(samples) >= 32766 / 32767)),
        "warning": "Reference-labelled measurements, not receive results or a unique physical impulse response.",
        "frames": [],
    }
    for frame in frames:
        selected = [r for r in rows if r["candidate"] == frame["candidate"]]
        if not selected or any(r["reference"] == "" for r in selected):
            raise ValueError("select an explicit diagnostic reference for unresolved headers first")
        bits = np.array([int(r["reference"]) for r in selected])
        start = float(frame["start_sample"]) + float(frame.get("equalizer_timing_offset_samples") or 0)
        period = float(frame.get("equalizer_period") or frame["initial_period"])
        # Use fixed spacing to distinguish channel response from decision timing
        # corrections; these measurements need not reproduce production BER.
        centers = start + (np.arange(len(bits)) + 0.5) * period
        z = []
        for hz, track in zip([1200, 2200], tracks):
            position = centers - 120
            index = np.arange(len(track))
            values = np.interp(position, index, track.real) + 1j * np.interp(position, index, track.imag)
            values *= np.exp(-2j * np.pi * hz * (480 / period - 1) * centers / 48000)
            z.append(values)
        z = np.array(z).T
        power = abs(z)**2 / 2
        measured = {
            "candidate": int(frame["candidate"]), "start_sample": start,
            "samples_per_symbol": period, "reference_bits": len(bits),
            "active_rms": rms(samples[round(start):round(start + len(bits) * period)]),
            "blocks": [], "memory_fits": [],
        }
        for begin in range(0, len(bits), 64):
            end = min(begin + 64, len(bits))
            block = power[begin:end]
            labels = bits[begin:end]
            measured["blocks"].append({
                "first_bit": begin, "end_bit": end,
                "desired_carrier_median_power": [float(np.median(block[labels == tone, tone])) if np.any(labels == tone) else None for tone in [0, 1]],
                "energy_ratio_quantiles_by_bit": [np.quantile((block[:, 1] / np.maximum(block[:, 0], 1e-20))[labels == tone], [0.1, 0.5, 0.9]).tolist() if np.any(labels == tone) else None for tone in [0, 1]],
            })
        for tone in [0, 1]:
            for depth in [0, 2, 4, 8]:
                design = np.array([[float(n >= lag and bits[n-lag] == tone) for lag in range(depth + 1)] + [1.] for n in range(len(bits))])
                values = z[:, tone]
                fitted = np.linalg.lstsq(design, values, rcond=None)[0]
                trained = np.linalg.lstsq(design[4:80], values[4:80], rcond=None)[0]
                variance = max(float(np.mean(abs(values-values.mean())**2)), 1e-20)
                later = values[80:]
                later_variance = max(float(np.mean(abs(later-later.mean())**2)), 1e-20)
                measured["memory_fits"].append({
                    "tone": tone, "memory_symbols": depth,
                    "all_symbols_reference_fitted_r_squared": float(1-np.mean(abs(values-design@fitted)**2)/variance),
                    "training_only_to_payload_r_squared": float(1-np.mean(abs(later-design[80:]@trained)**2)/later_variance),
                    "all_symbols_fitted_tap_magnitudes": abs(fitted[:-1]).tolist(),
                })
        result["frames"].append(measured)
    for left, right in zip(result["frames"], result["frames"][1:]):
        end = round(left["start_sample"] + left["reference_bits"] * left["samples_per_symbol"])
        next_start = round(right["start_sample"])
        left["gap_to_next_s"] = (next_start - end) / 48000
        left["gap_first_half_rms"] = rms(samples[end:(end + next_start)//2])
        left["gap_second_half_rms"] = rms(samples[(end + next_start)//2:next_start])
    args.output.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
