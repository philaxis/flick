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


def position(row, color, bounds=(1, 1918)):
    if int(row[color + '_n']) < (2 if color=='marker' else 32) or int(row[color + '_l']) <= bounds[0] or int(row[color + '_r']) >= bounds[1]:
        return None
    return float(row.get(color + '_x', (float(row[color + '_l']) + float(row[color + '_r'])) / 2))


def cv(values):
    return st.pstdev(values) / abs(st.mean(values)) * 100 if len(values) > 1 and abs(st.mean(values)) > 1e-8 else math.nan


def report(folder):
    meta = json.loads((folder / 'display-meta.json').read_text())
    frequency, period = meta['frequency'], meta['refresh_ticks']
    print(f"Refresh: {frequency / period:.4f} Hz. Composed pixels, not a photodiode or physical panel scanout.")
    print('|scenario|updates|capture losses|pixel repeats / pairs|no-update refreshes|abs step CV %|first response ms|input abs step CV %|marker error p95 px|max adjacent step px|post-dead-zone onset ms|input interval p95 ms|')
    print('|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|')
    data = []
    for path in sorted(folder.glob('*-display.csv')):
        label = path.name.removesuffix('-display.csv')
        frames, inputs = read(path), read(folder / (label + '-input.csv'))
        times = [int(r['qpc']) for r in inputs]
        start, end = times[0], times[-1]
        bounds=(55,1864) if label.startswith('board') else (1,meta.get('width',1920)-2)
        pos=lambda r,c: position(r,c,bounds)
        dead=0.25 if '-ptp-' in label else 0.012
        eligible=next((int(r['qpc']) for r in inputs if abs(float(r['hand_cells']))>dead),end)
        intervals=[(b-a)*1000/frequency for a,b in zip(times,times[1:])]
        cadence_p95=sorted(intervals)[int(len(intervals)*.95)] if intervals else math.nan
        # Sample actual quantized reports at every refresh, including unchanged desktop frames.
        quantum=1200 if '-ptp-' in label else 1000
        uniform=[]
        for t in range(start,end,int(period)):
            i=max(0,bisect.bisect_right(times,t)-1)
            uniform.append(round(float(inputs[i]['hand_cells'])*quantum)/quantum)
        input_cv=cv([abs(b-a) for a,b in zip(uniform,uniform[1:])])
        active = [r for r in frames if start <= int(r['qpc']) <= end]
        losses = sum(max(0, int(r['accumulated']) - 1) for r in active)
        repeats = gaps = 0
        adjacent_changes=[]
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
                          if pos(a,c) is not None and pos(b,c) is not None]
            if not candidates:
                continue
            c = max(candidates)[1]
            dx = pos(b,c)-pos(a,c)
            changes.append(dx)
            if abs((t1-t0)/period-1)<0.25: adjacent_changes.append(dx)
            repeats += abs(dx) < 0.01
            gaps += max(0, round((t1-t0)/period)-1)
            if int(b['marker_n']) and pos(b,'magenta') is not None:
                marker_errors.append(abs(pos(b,'magenta')-pos(b,'marker')))
        first = None
        initial = next((r for r in frames if int(r['qpc']) >= start and pos(r,'magenta') is not None), None)
        if initial:
            base = pos(initial,'magenta')
            changed = next((r for r in active if pos(r,'magenta') is not None and abs(pos(r,'magenta')-base) >= 1), None)
            if changed:
                first = (int(changed['qpc'])-start)*1000/frequency
        settling=[r for r in frames if end<int(r['qpc'])<=end+frequency*0.8]
        settle_steps=[]
        for a,b in zip(settling,settling[1:]):
            if int(b['accumulated'])>1 or abs((int(b['qpc'])-int(a['qpc']))/period-1)>0.25: continue
            for c in COLORS:
                if pos(a,c) is not None and pos(b,c) is not None: settle_steps.append(abs(pos(b,c)-pos(a,c)))
        p95 = sorted(marker_errors)[int(len(marker_errors)*.95)] if marker_errors else math.nan
        line = dict(scenario=label, updates=len(active), capture_losses=losses, repeat_pairs=repeats, pairs=len(changes), no_update_refreshes=gaps,
                    step_cv=cv(list(map(abs,changes))), first_response_ms=first, input_step_cv=input_cv, marker_error_p95=p95, max_step_px=max(map(abs,adjacent_changes),default=0), eligible_response_ms=None if first is None else first-(eligible-start)*1000/frequency, input_interval_p95_ms=cadence_p95, settling_max_step_px=max(settle_steps,default=0))
        data.append(line)
        first_text = f'{first:.1f}' if first is not None else 'missing'
        onset=f'{line["eligible_response_ms"]:.1f}' if first is not None else 'missing'
        print(f'|{label}|{len(active)}|{losses}|{repeats}/{len(changes)}|{gaps}|{line["step_cv"]:.1f}|{first_text}|{input_cv:.1f}|{p95:.2f}|{line["max_step_px"]:.1f}|{onset}|{cadence_p95:.2f}|')
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
