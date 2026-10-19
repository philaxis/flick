#!/usr/bin/env python3
"""Summarize measured GPU engine time; do not call it isolated frame/pass time."""
import csv
import json
from pathlib import Path
import sys


def engine_time(rows, hz):
    valid = [r for r in rows if r['status'] == '0']
    if len(valid) < 2 or any(r['type'] != 'Timer100Ns' for r in valid):
        return None
    first, last = valid[0], valid[-1]
    ticks = int(last['timestamp100ns']) - int(first['timestamp100ns'])
    busy = int(last['raw']) - int(first['raw'])
    if ticks <= 0 or busy < 0:
        return None
    seconds = ticks / 10_000_000
    return dict(seconds=seconds, engine_ms=busy / 10_000,
                engine_ms_per_refresh=(busy / 10_000) / (seconds * hz))


def report(folder):
    meta = json.loads((folder / 'display-meta.json').read_text())
    hz = meta['frequency'] / meta['refresh_ticks']
    results = []
    for path in sorted(folder.glob('*-gpu.csv')):
        with path.open(encoding='utf-8-sig') as f:
            rows = list(csv.DictReader(f))
        groups = {}
        for row in rows:
            groups.setdefault((row['owner'], row['instance']), []).append(row)
        totals = {}
        for (owner, instance), values in groups.items():
            result = engine_time(values, hz)
            if result:
                totals[owner] = totals.get(owner, 0) + result['engine_ms_per_refresh']
        results.append(dict(scenario=path.name.removesuffix('-gpu.csv'), engine_ms_per_refresh=totals,
                            counter_types=sorted({r['type'] for r in rows})))
    for path in sorted(folder.glob('*-gpu.csv.error.txt')):
        results.append(dict(scenario=path.name.removesuffix('-gpu.csv.error.txt'), error=path.read_text(encoding='utf-8-sig').strip()))
    print(json.dumps(results, indent=2))
    (folder / 'gpu-summary.json').write_text(json.dumps(results, indent=2))


if __name__ == '__main__':
    samples = [dict(status='0', type='Timer100Ns', timestamp100ns=str(t), raw=str(v))
               for t, v in [(10_000_000, 100), (20_000_000, 600_100)]]
    assert engine_time(samples, 60)['engine_ms_per_refresh'] == 1
    assert engine_time([dict(samples[0], type='RateOfCountsPerSecond64'), samples[1]], 60) is None
    for arg in sys.argv[1:]:
        report(Path(arg))
