import { readFileSync } from 'node:fs'
import { beforeAll, describe, expect, it } from 'vitest'
import { Attr, Modifier, TermyCore, attributes, builtinTheme, builtinThemeIds, decodeColor, glyphPlan, initSync } from '../src/index.ts'

beforeAll(() => {
  initSync(readFileSync(new URL('../src/wasm/termy_wasm_bg.wasm', import.meta.url)))
})

describe('TermyCore', () => {
  it('parses output into cells', () => {
    const term = new TermyCore({ cols: 20, rows: 4 })
    term.write('hello\r\n\x1b[1;38;2;255;0;0mred')
    const rows = term.readRows()
    expect(term.lineText(0)).toBe('hello')
    expect(rows.text(1, 0)).toBe('r')
    const offset = rows.offset(1, 0)
    expect(decodeColor(rows.data[offset + 1]!)).toEqual({ kind: 'rgb', rgb: 0xff0000 })
    expect(attributes(rows.data[offset + 4]!) & Attr.Bold).toBe(Attr.Bold)
    expect(term.cursor()).toMatchObject({ row: 1, col: 3, visible: true, shape: 'block' })
    term.dispose()
  })

  it('reports events, replies and modes', () => {
    const term = new TermyCore({ cols: 20, rows: 4 })
    term.write('\x1b]0;my title\x07\x07\x1b[6n\x1b[?2004h\x1b[>1u')
    expect(term.takeEvents()).toEqual([{ type: 'title', title: 'my title' }, { type: 'bell' }])
    expect(new TextDecoder().decode(term.takeReplies())).toBe('\x1b[1;1R')
    expect(term.modes()).toMatchObject({ bracketedPaste: true, kittyKeyboardFlags: 1 })
    expect(term.encodeKey('escape', undefined, 0)).toEqual(new TextEncoder().encode('\x1b[27u'))
    expect(term.encodeKey('c', 'c', Modifier.Ctrl)).toEqual(new TextEncoder().encode('\x1b[99;5u'))
  })

  it('decodes kitty graphics placements', () => {
    const term = new TermyCore({ cols: 10, rows: 4 })
    term.setCellPixels(10, 20)
    term.write('\x1b_Ga=T,f=32,s=1,v=1,i=3;/wAA/w==\x1b\\')
    const [placement] = term.readGraphics(10, 20)
    expect(placement).toMatchObject({ imageId: 3, imageWidth: 1, imageHeight: 1, viewportRow: 0, col: 0 })
    expect(placement!.draw).toEqual({ left: 0, top: 0, width: 1, height: 1 })
    expect(term.graphicsImage(0)).toEqual({ format: 'rgba', data: new Uint8Array([255, 0, 0, 255]) })
  })

  it('tracks damage and scrollback', () => {
    const term = new TermyCore({ cols: 5, rows: 2, scrollback: 10 })
    term.takeDamage()
    term.write('a\r\nb\r\nc')
    expect(term.historySize).toBe(1)
    expect(term.lineText(-1)).toBe('a')
    expect(term.scrollDisplay(1)).toBe(true)
    expect(term.readRows(0, 1).text(0, 0)).toBe('a')
  })
})

describe('themes and glyphs', () => {
  it('ships Termy themes', () => {
    expect(builtinThemeIds()).toContain('tokyo-night')
    const theme = builtinTheme('Tokyo Night')!
    expect(theme.ansi).toHaveLength(16)
    expect(theme.background).toMatch(/^#[0-9a-f]{6}$/)
  })

  it('plans box drawing geometry', () => {
    const plan = glyphPlan(0x2500, [0, 0, 0, 0], 9, 18, 14)!
    expect(plan.kind).toBe('box')
    expect(glyphPlan(0x61, [0, 0, 0, 0], 9, 18, 14)).toBeUndefined()
  })
})

describe('OSC 7501 program status', () => {
  it('exposes inherited snapshots and expires only active records on process exit', () => {
    const term = new TermyCore({ cols: 20, rows: 4 })
    term.write('\x1b]7501;?\x07\x1b]7501;state=working:app=deploy\x07')
    term.write('\x1b]7501;state=done:id=child:msg=SGk=\x1b\\')
    expect(new TextDecoder().decode(term.takeReplies())).toBe('\x1b]7501;?\x1b\\')
    const events = term.takeEvents()
    expect(events).toHaveLength(1)
    expect(events[0]).toMatchObject({ type: 'programStatus', records: [
      { id: '', state: 'working', app: 'deploy', progress: null },
      { id: 'child', state: 'done', app: 'deploy', msg: 'Hi' },
    ] })
    term.processExited()
    expect(term.takeEvents()).toMatchObject([{ type: 'programStatus', records: [
      { id: 'child', state: 'done', app: null, msg: 'Hi' },
    ] }])
    term.write('\x1bc')
    expect(term.takeEvents()).toContainEqual({ type: 'programStatus', records: [] })
    expect(term.takeEvents()).toEqual([])
    term.dispose()
  })
})
