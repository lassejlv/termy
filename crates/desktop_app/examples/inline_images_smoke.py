#!/usr/bin/env python3
"""Run inside Termy on Unix to check Kitty/iTerm2 images and cursor replies.

Images remain visible for manual resizing. Pass --report PATH to save results.
"""
import argparse
import base64
import json
import os
import select
import struct
import sys
import termios
import time
import tty
import zlib
from pathlib import Path

ESC = b'\x1b'
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--report', type=Path)
args = parser.parse_args()
checks = {}

def send(data):
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

def at(row, col, text=''):
    send(f'\x1b[{row};{col}H{text}'.encode())

def read_until(end):
    data = b''
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline:
        if select.select([sys.stdin], [], [], .1)[0]:
            data += os.read(sys.stdin.fileno(), 4096)
            if end in data:
                return data
    raise AssertionError(f'terminal reply timed out: {data!r}')

def cursor(expected, name):
    send(ESC + b'[6n')
    reply = read_until(b'R')
    actual = reply.split(b'\x1b[')[-1].decode().rstrip('R')
    checks[name] = actual
    assert actual == expected, (name, actual, expected)

def kitty(control, data=b'', unpadded=False):
    payload = base64.b64encode(data)
    if unpadded:
        payload = payload.rstrip(b'=')
    send(ESC + b'_G' + control.encode() + b';' + payload + ESC + b'\\')

def png(width, height):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    rows = b''.join(b'\0' + bytes(c for x in range(width) for c in
        ((30, 130, 245, 255) if (x // 8 + y // 8) % 2 else (245, 165, 35, 255)))
        for y in range(height))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b'')

old = termios.tcgetattr(sys.stdin)
try:
    tty.setraw(sys.stdin.fileno())
    send(ESC + b'[2J' + ESC + b'[H' + ESC + b'[?25l')
    at(1, 3, 'TERMY / INLINE IMAGE SMOKE TEST')
    at(3, 3, 'Kitty: 16 x 3 cells, cursor advances')
    rgba = bytes(c for y in range(40) for x in range(80) for c in
        ((235, 70, 105, 255) if (x // 10 + y // 10) % 2 else (50, 205, 150, 255)))
    at(5, 3)
    kitty('a=T,i=501,f=32,s=80,v=40,c=16,r=3', rgba)
    checks['kitty_ack'] = read_until(ESC + b'\\').decode()
    assert 'i=501;OK' in checks['kitty_ack']
    cursor('7;19', 'kitty_advance')
    at(9, 3, 'Kitty: unpadded payload, C=1 keeps cursor')
    at(10, 3)
    kitty('a=T,i=502,f=32,s=80,v=40,c=16,r=2,C=1,q=2', rgba, unpadded=True)
    cursor('10;3', 'kitty_preserve')
    at(13, 3, 'iTerm2: stretched 16 x 3 cells')
    at(15, 3)
    image = png(80, 40)
    send(ESC + b']1337;File=inline=1;width=16;height=3;preserveAspectRatio=0:' + base64.b64encode(image) + b'\x07')
    cursor('17;19', 'iterm2_advance')
    at(13, 38, 'iTerm2: aspect ratio, cursor kept')
    at(15, 38)
    send(ESC + b']1337;File=inline=1;width=16;height=3;doNotMoveCursor=1:' + base64.b64encode(image) + b'\x07')
    cursor('15;38', 'iterm2_preserve')
    at(21, 3, 'PASS: image acknowledgements and cursor positions')
    at(23, 3, 'Resize the window to check image geometry.')
    at(25, 3)
    checks['environment'] = {k: os.environ.get(k) for k in ('TERM_PROGRAM', 'TERM_PROGRAM_VERSION', 'TERMY_TERM_PROGRAM')}
    checks['passed'] = True
finally:
    send(ESC + b'[?25h')
    termios.tcsetattr(sys.stdin, termios.TCSADRAIN, old)
    if args.report:
        args.report.write_text(json.dumps(checks, indent=2) + '\n')
