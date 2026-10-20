"""Turns what scripts/landing-cells.ps1 captured into the landing page's cells.

    python3 scripts/landing-cells.py <cells-out> docs/cells/<ko|en>

Blurs the taskbar's clock (the real time and date), sizes and compresses the
pictures, and writes cells.js, the desktops docs/<ko|en>/index.html shows.
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
SHELVES = [['d00', 'd01', 'd02'], ['d10', 'd11'], ['d20', 'd21', 'd22']]   # building, a break, marketing
OWN = {'d21'}            # the desktop the board opens on: its windows by themselves
DESK_W, Q = 1920, 80


def clock_hidden(im):
    """The clock and date at the taskbar's right end, blurred past reading."""
    box = (sw - 140, work['h'] + 2, sw, sh)
    im.paste(im.crop(box).filter(ImageFilter.GaussianBlur(14)), box[:2])
    return im


# Other people's thumbnails on YouTube, softened: (left, top, right, bottom) as fractions of the browser window.
SOFTEN = {'d10': [(.70, .17, .99, 1)], 'd11': [(.08, .26, .49, 1)]}
FRAMES = {'d10': ['d10', 'd10b', 'd10c']}   # the video, taken again as it plays


def softened(d, im):
    f = layout['scenes'][d]['windows'][0]['frame']
    for l, t, r, b in SOFTEN.get(d, []):
        box = tuple(round(v) for v in (f['x'] + l * f['w'], f['y'] + t * f['h'], f['x'] + r * f['w'], min(f['y'] + b * f['h'], work['h'])))
        im.paste(im.crop(box).filter(ImageFilter.GaussianBlur(10)), box[:2])
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
        pics = [save(softened(d, clock_hidden(Image.open(src / f'{n}.png').convert('RGB'))), n, DESK_W) for n in FRAMES.get(d, [d])]
        img = pics[0]
        wins = []
        for i, w in enumerate(layout['scenes'][d]['windows']):
            win = {'app': w['app'], 'r': rect(w['frame'])}
            if d in OWN:
                win['img'] = save(window(src / f'w-{d}-{i}.png'), f'w-{d}-{i}', round(w['frame']['w'] * .8), 80)
            wins.append(win)
        out.append({'img': img, 'wins': wins, **({'frames': pics} if d in FRAMES else {})})
    shelves.append(out)

pins = layout['scenes']['pins']['windows']
pin = lambda i: {'app': pins[i]['app'], 'r': rect(pins[i]['frame']),
                 'img': save(window(src / f'w-pins-{i}.png'), f'w-pins-{i}', round(pins[i]['frame']['w'] * .8), 82)}
cells = {'sw': sw, 'sh': sh, 'shelves': shelves, 'pinAll': pin(0), 'pinRow': {'rows': [0, 2], **pin(1)}}

for p in (src / 'icons').glob('*.png'):
    shutil.copy(p, dst / 'icons' / p.name)
# A page opened as an app of its own is Edge's.
shutil.copy(src / 'icons' / 'edge.png', dst / 'icons' / 'app.png')
# The new Paint, Notepad and Calculator keep their icons out of their exes; their title bars have them.
for app, shot, box in (('paint', 'w-d22-0', (9, 12, 29, 32)), ('calc', 'w-pins-0', (23, 11, 43, 31)), ('notepad', 'w-d20-1', (20, 21, 40, 41))):
    Image.open(src / f'{shot}.png').crop(box).resize((32, 32), Image.LANCZOS).save(dst / 'icons' / f'{app}.png')
total = sum(p.stat().st_size for p in dst.rglob('*') if p.is_file())
(dst / 'cells.js').write_text('// The desktops the demo shows, made by scripts/landing-cells.py.\n(window.FLICK = window.FLICK || {}).cells = '
                             + json.dumps(cells, ensure_ascii=False, separators=(',', ':')) + ';\n', encoding='utf-8')
print(f'// {total / 1024:.0f} KB', file=sys.stderr)
