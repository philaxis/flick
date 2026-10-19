#!/usr/bin/env python3
"""Summarize drive's six-second pacing probe: frame starts, not display receipts."""
import csv
import math
from pathlib import Path
import re
import statistics
import sys


def stats(gaps, period):
    ordered = sorted(gaps)
    assert ordered and period > 0
    percentile = lambda p: ordered[max(0, math.ceil(len(ordered) * p) - 1)]
    missed = sum(max(0, math.floor(gap / period + 0.5) - 1) for gap in ordered)
    return (len(ordered), statistics.mean(ordered), percentile(0.5), percentile(0.95), max(ordered),
            missed, missed / (len(ordered) + missed) * 100)


def report(folder):
    hz = float(re.search(r'refresh_hz=([\d.]+)', (folder / 'report.txt').read_text(encoding='utf-8-sig')).group(1))
    period = 1000 / hz
    print(f'{folder}: {hz:.4f} Hz, period {period:.4f} ms, p95 budget {period * 1.25:.4f} ms')
    for mode in ['board', 'follow']:
        with (folder / f'{mode}.csv').open() as source:
            rows = list(csv.DictReader(source))
        start = float(rows[0]['qpc_ms'])
        # Same window for both builds: exclude 0.5s startup and the screenshot/tail.
        active = [r for r in rows if 500 <= float(r['qpc_ms']) - start <= 5500]
        gap = 'interval_ms' if mode == 'board' else 'gap_ms'
        gaps = [float(r[gap]) for r in active if r[gap] and float(r[gap]) > 0]
        n, mean, p50, p95, maximum, missed, rate = stats(gaps, period)
        print(f'{mode}: n={n} mean={mean:.3f} p50={p50:.3f} p95={p95:.3f} max={maximum:.3f} ms estimated_missing={missed} ({rate:.2f}%)')
        for column in ['draw_ms', 'dwm_ms', 'submit_ms', 'thumbnail_updates', 'work_ms', 'thumbnail_ms']:
            if column in rows[0]:
                values = sorted(float(r[column]) for r in active)
                print(f'  {column}: mean={statistics.mean(values):.3f} p95={values[math.ceil(len(values) * .95) - 1]:.3f} max={max(values):.3f}')
        first = [float(r[gap]) for r in rows[:12] if r[gap]]
        print(f'  first_12_gaps_ms={first}')


if __name__ == '__main__':
    assert stats([10, 10, 20], 10) == (3, 40 / 3, 10, 20, 20, 1, 25)
    for argument in sys.argv[1:]:
        report(Path(argument))
