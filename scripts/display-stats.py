#!/usr/bin/env python3
"""Desktop Duplication evidence. Never count accumulated captures as good frames.

CV is descriptive, not a pacing pass/fail: Flick's unchanged F curve deliberately
slows at a latch and stops after commits. Pixel repeats below 1px of intended
travel are not distinguishable from raster quantization with bounding boxes.
"""
import bisect
import csv
import json
import math
from pathlib import Path
import statistics as st
import sys

COLORS = ('magenta', 'cyan', 'yellow')


def read(path):
    with path.open(encoding='utf-8-sig') as f:
        return list(csv.DictReader(f))


def position(row, color):
    if int(row[color + '_n']) < (2 if color=='marker' else 32) or int(row[color + '_l']) <= 1 or int(row[color + '_r']) >= 1918:
        return None
    return float(row.get(color + '_x', (float(row[color + '_l']) + float(row[color + '_r'])) / 2))


def cv(values):
    return st.pstdev(values) / abs(st.mean(values)) * 100 if len(values) > 1 and abs(st.mean(values)) > 1e-8 else math.nan


def report(folder):
    meta = json.loads((folder / 'display-meta.json').read_text())
    frequency, period = meta['frequency'], meta['refresh_ticks']
    print(f"Refresh: {frequency / period:.4f} Hz. Composed pixels, not a photodiode or physical panel scanout.")
    print('|scenario|updates|capture losses|pixel repeats / pairs|no-update refreshes|step CV %|first response ms|input step CV %|marker error p95 px|')
    print('|---|---:|---:|---:|---:|---:|---:|---:|---:|')
    data = []
    for path in sorted(folder.glob('*-display.csv')):
        label = path.name.removesuffix('-display.csv')
        frames, inputs = read(path), read(folder / (label + '-input.csv'))
        times = [int(r['qpc']) for r in inputs]
        start, end = times[0], times[-1]
        active = [r for r in frames if start <= int(r['qpc']) <= end]
        losses = sum(max(0, int(r['accumulated']) - 1) for r in active)
        repeats = gaps = 0
        changes, raw_steps, marker_errors = [], [], []
        for a, b in zip(active, active[1:]):
            t0, t1 = int(a['qpc']), int(b['qpc'])
            if int(b['accumulated']) > 1:
                continue  # These pixels are unknown; cannot diagnose a stall.
            i0, i1 = [max(0, bisect.bisect_right(times, t) - 1) for t in (t0, t1)]
            hand = float(inputs[i1]['hand_cells']) - float(inputs[i0]['hand_cells'])
            if abs(hand) < 1e-7:
                continue  # Deliberate pauses are not rendering stalls.
            raw_steps.append(hand)
            # Prefer the largest fully visible patch common to both frames.
            candidates = [(min(int(a[c+'_n']), int(b[c+'_n'])), c) for c in COLORS
                          if position(a,c) is not None and position(b,c) is not None
                          and abs(int(a[c+'_r'])-int(a[c+'_l'])-int(b[c+'_r'])+int(b[c+'_l'])) <= 2]
            if not candidates:
                continue
            c = max(candidates)[1]
            dx = position(b,c)-position(a,c)
            changes.append(dx)
            repeats += abs(dx) < 0.01
            gaps += max(0, round((t1-t0)/period)-1)
            if int(b['marker_n']) and position(b,'magenta') is not None:
                marker_errors.append(abs(position(b,'magenta')-position(b,'marker')))
        first = None
        initial = next((r for r in frames if int(r['qpc']) >= start and position(r,'magenta') is not None), None)
        if initial:
            base = position(initial,'magenta')
            changed = next((r for r in active if position(r,'magenta') is not None and abs(position(r,'magenta')-base) >= 1), None)
            if changed:
                first = (int(changed['qpc'])-start)*1000/frequency
        p95 = sorted(marker_errors)[int(len(marker_errors)*.95)] if marker_errors else math.nan
        line = dict(scenario=label, updates=len(active), capture_losses=losses, repeat_pairs=repeats, pairs=len(changes), no_update_refreshes=gaps,
                    step_cv=cv(changes), first_response_ms=first, input_step_cv=cv(raw_steps), marker_error_p95=p95, max_step_px=max(map(abs,changes),default=0))
        data.append(line)
        first_text = f'{first:.1f}' if first is not None else 'missing'
        print(f'|{label}|{len(active)}|{losses}|{repeats}/{len(changes)}|{gaps}|{cv(changes):.1f}|{first_text}|{cv(raw_steps):.1f}|{p95:.2f}|')
    (folder / 'display-summary.json').write_text(json.dumps(data, indent=2))
    print('CV includes intentional F-curve latching and reversal; it is not residual judder. First response is motion onset, not steady-state latency.')
    return data


def self_check():
    assert cv([3,3,3]) == 0
    assert cv([0,2]) == 100
    assert position({'magenta_n':'100','magenta_l':'10','magenta_r':'20'},'magenta') == 15
    assert position({'magenta_n':'0'},'magenta') is None


if __name__ == '__main__':
    self_check()
    for arg in sys.argv[1:]:
        report(Path(arg))
