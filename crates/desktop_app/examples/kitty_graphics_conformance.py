#!/usr/bin/env python3
"""Interactive Kitty graphics smoke test (Python standard library only).

Run in Termy: python3 crates/desktop_app/examples/kitty_graphics_conformance.py
Space switches between layout/animation and scrolling. R redraws; Q exits.
Pass --sync to wrap each redraw in a synchronized update, as Grok Build does.
The test uses an alternate screen and restores the terminal on exit.
"""
import base64
import os
import select
import signal
import sys
import termios
import tty
import zlib

ESC = '\x1b'
MARKS = [0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D,
         0x033E, 0x033F, 0x0346, 0x034A, 0x034B, 0x034C]
PAGE = int('--scroll' in sys.argv)
REDRAW = True


def write(value):
    sys.stdout.write(value)


def at(row, col, value=''):
    write(f'{ESC}[{row};{col}H{value}')


def command(control, payload=b''):
    write(f'{ESC}_G{control};{base64.b64encode(payload).decode()}{ESC}\\')


def pixels(width, height, colors=((244, 91, 105), (60, 207, 159))):
    return bytes(channel for y in range(height) for x in range(width)
                 for channel in (*colors[(x * 2 // width + y * 2 // height) % 2], 255))


def upload(image_id, width, height, data, extra='', compress=False):
    if compress:
        data = zlib.compress(data)
        extra += ',o=z'
    encoded = base64.b64encode(data).decode()
    chunks = [encoded[i:i + 4096] for i in range(0, len(encoded), 4096)]
    for index, chunk in enumerate(chunks):
        more = int(index + 1 < len(chunks))
        control = (f'a=t,f=32,s={width},v={height},i={image_id},q=2{extra},m={more}'
                   if index == 0 else f'm={more}')
        write(f'{ESC}_G{control};{chunk}{ESC}\\')


def put(image_id, row, col, extra=''):
    at(row, col)
    command(f'a=p,i={image_id},C=1,q=2{extra}')


def placeholder(image_id, row, col, gap=True):
    for y in range(3):
        at(row + y, col)
        write(f'{ESC}[38;2;0;0;{image_id}m')
        for x in range(10):
            if gap and y == 1 and 3 <= x <= 5:
                write(' ')
            else:
                write(chr(0x10EEEE) + chr(MARKS[y]) + chr(MARKS[x]))
        write(f'{ESC}[0m')


def layout():
    at(1, 2, 'KITTY GRAPHICS  /  layout, layering, placeholders, animation')
    at(3, 2, 'Natural 88 x 44')
    at(3, 28, 'Width 12 cells / ratio 2:1')
    at(3, 58, 'Crop + pixel offset')
    upload(41, 88, 44, pixels(88, 44))
    put(41, 4, 2)
    put(41, 4, 28, ',c=12')
    put(41, 4, 58, ',x=22,y=11,w=44,h=22,c=10,r=3,X=3,Y=4')

    at(9, 2, 'Image above cell background, below text')
    for row in range(10, 13):
        at(row, 2, f'{ESC}[44;97mTEXT OVER IMAGE {ESC}[0m')
    put(41, 10, 2, ',c=16,r=3,z=-1')
    at(9, 45, 'Image below explicit backgrounds')
    for row in range(10, 13):
        at(row, 45, f'{ESC}[44;97mBLUE{ESC}[0m default')
    put(41, 10, 45, ',c=16,r=3,z=-2000000000')

    at(15, 2, 'Placeholders: two copies with gaps')
    command('a=p,i=41,p=12,U=1,c=10,r=3,q=2')
    placeholder(41, 16, 2)
    placeholder(41, 16, 18)
    at(15, 45, 'Relative placement clipped at left edge')
    put(41, 16, 50, ',p=13,c=4,r=2')
    put(41, 1, 1, ',p=14,P=41,Q=13,H=-52,V=0,c=4,r=2')

    at(22, 2, 'Animation: red / green / blue, 350 ms per frame')
    upload(42, 70, 40, pixels(70, 40, ((238, 70, 80), (160, 30, 50))), compress=True)
    put(42, 23, 2, ',c=14,r=4')
    command('a=a,i=42,r=1,z=350,q=2')
    for colors in [((40, 210, 100), (15, 135, 75)), ((55, 135, 245), (25, 60, 180))]:
        data = zlib.compress(pixels(70, 40, colors))
        command('a=f,i=42,f=32,s=70,v=40,o=z,z=350,q=2', data)
    command('a=a,i=42,s=3,v=1,q=2')
    at(29, 2, 'Resize: cell-sized images follow cells; natural image stays 88 x 44.')


def scrolling():
    at(1, 2, 'KITTY GRAPHICS  /  scroll region and final-chunk cursor')
    upload(51, 100, 80, pixels(100, 80))
    put(51, 5, 5, ',c=12,r=4')
    at(3, 2, 'The upper image moves up two rows and loses its top two rows.')
    write(f'{ESC}[5;10r')
    at(10, 1)
    write('\n')
    write(f'{ESC}[r')
    at(14, 2, 'Footer image remains here when rows 5-10 scroll.')
    put(51, 15, 5, ',c=12,r=3')
    write(f'{ESC}[5;10r')
    at(10, 1)
    write('\n')
    write(f'{ESC}[r')
    at(21, 2, 'Chunked upload begins at column 5; final chunk displays at column 30.')
    at(23, 5)
    command('a=T,i=52,f=32,s=1,v=1,c=8,r=3,C=1,q=2,m=1', bytes([240, 160, 50]))
    at(23, 30)
    command('m=0', bytes([255]))


def redraw():
    if '--sync' in sys.argv:
        write(f'{ESC}[?2026h')
    write(f'{ESC}[r{ESC}[0m{ESC}[2J{ESC}[H')
    command('a=d,d=A,q=2')
    (layout if PAGE == 0 else scrolling)()
    size = os.get_terminal_size()
    at(min(size.lines, 33), 2, f'Space: next page   R: redraw   Q: quit   {size.columns} x {size.lines} cells')
    if '--sync' in sys.argv:
        write(f'{ESC}[?2026l')
    sys.stdout.flush()


def resized(*_):
    global REDRAW
    REDRAW = True


def main():
    global PAGE, REDRAW
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise SystemExit('Run this demo inside a terminal.')
    original = termios.tcgetattr(sys.stdin)
    signal.signal(signal.SIGWINCH, resized)
    try:
        tty.setraw(sys.stdin.fileno())
        write(f'{ESC}[?1049h{ESC}[?25l')
        while True:
            if REDRAW:
                REDRAW = False
                redraw()
            ready, _, _ = select.select([sys.stdin], [], [], 0.1)
            if ready:
                key = os.read(sys.stdin.fileno(), 1)
                if key in (b'q', b'Q', b'\x03', b'\x04'):
                    break
                if key == b' ':
                    PAGE = 1 - PAGE
                    REDRAW = True
                if key in (b'r', b'R'):
                    REDRAW = True
    finally:
        command('a=d,d=A,q=2')
        write(f'{ESC}[r{ESC}[0m{ESC}[?25h{ESC}[?1049l')
        sys.stdout.flush()
        termios.tcsetattr(sys.stdin, termios.TCSADRAIN, original)


if __name__ == '__main__':
    main()
