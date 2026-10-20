"""Turns what scripts/landing-cells.ps1 captured into the landing page's cells.

    python3 scripts/landing-cells.py <cells-out> docs/cells

Blurs the taskbar's clock (the real time and date), sizes and compresses the
pictures, and prints the CELLS constant docs/index.html holds.
"""
import json, shutil, sys
from pathlib import Path
from PIL import Image, ImageFilter

src, dst = Path(sys.argv[1]), Path(sys.argv[2])
dst.mkdir(parents=True, exist_ok=True)
(dst / 'icons').mkdir(exist_ok=True)
layout = json.loads((src / 'layout.json').read_text(encoding='utf-8-sig'))
sw, sh = layout['screen']['w'], layout['screen']['h']
work = layout['work']
SHELVES = [['d00', 'd01', 'd02'], ['d10', 'd11'], ['d20', 'd21']]
OWN = {'d10'}            # the desktop the board opens on: its windows by themselves
DESK_W, Q = 1920, 80


def clock_hidden(im):
    """The clock and date at the taskbar's right end, blurred past reading."""
    box = (sw - 140, work['h'] + 2, sw, sh)
    im.paste(im.crop(box).filter(ImageFilter.GaussianBlur(14)), box[:2])
    return im


def dates_hidden(d, im):
    """Explorer's "date modified" column is today's real date and time too."""
    for w in layout['scenes'][d]['windows']:
        if w['app'] == 'explorer' and d == 'd02':
            f = w['frame']
            box = (f['x'] + 538, f['y'] + 208, f['x'] + 716, f['y'] + 432)
            im.paste(im.crop(box).filter(ImageFilter.GaussianBlur(9)), box[:2])
    return im


def window(path):
    """A window cut out by itself, less the one pixel of what was behind its edge."""
    im = Image.open(path).convert('RGB')
    return im.crop((1, 1, im.width - 1, im.height - 1))


def save(im, name, width, q=Q):
    if im.width > width:
        im = im.resize((width, round(im.height * width / im.width)), Image.LANCZOS)
    im.save(dst / f'{name}.webp', 'WEBP', quality=q, method=6)
    return f'{name}.webp'


def rect(f):
    return [f['x'], f['y'], f['w'], f['h']]


shelves = []
for row in SHELVES:
    out = []
    for d in row:
        img = save(dates_hidden(d, clock_hidden(Image.open(src / f'{d}.png').convert('RGB'))), d, DESK_W)
        wins = []
        for i, w in enumerate(layout['scenes'][d]['windows']):
            win = {'app': w['app'], 'r': rect(w['frame'])}
            if d in OWN:
                win['img'] = save(window(src / f'w-{d}-{i}.png'), f'w-{d}-{i}', round(w['frame']['w'] * .8), 80)
            wins.append(win)
        out.append({'img': img, 'wins': wins})
    shelves.append(out)

pins = layout['scenes']['pins']['windows']
pin = lambda i: {'app': pins[i]['app'], 'r': rect(pins[i]['frame']),
                 'img': save(window(src / f'w-pins-{i}.png'), f'w-pins-{i}', round(pins[i]['frame']['w'] * .8), 82)}
cells = {'sw': sw, 'sh': sh, 'shelves': shelves, 'pinAll': pin(0), 'pinRow': {'row': 1, **pin(1)}}

for p in (src / 'icons').glob('*.png'):
    shutil.copy(p, dst / 'icons' / p.name)
# The new Paint, Notepad and Calculator keep their icons out of their exes; their title bars have them.
for app, shot, box in (('paint', 'w-d11-0', (9, 12, 29, 32)), ('calc', 'w-pins-0', (23, 11, 43, 31)), ('notepad', 'w-pins-1', (20, 21, 40, 41))):
    Image.open(src / f'{shot}.png').crop(box).resize((32, 32), Image.LANCZOS).save(dst / 'icons' / f'{app}.png')
total = sum(p.stat().st_size for p in dst.rglob('*') if p.is_file())
print(f'const CELLS = {json.dumps(cells, ensure_ascii=False, separators=(",", ":"))};')
print(f'// {total / 1024:.0f} KB', file=sys.stderr)
