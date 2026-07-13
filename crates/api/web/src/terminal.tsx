// In-browser sandbox terminal.
//
// The browser connects to the Termy API's terminal relay (same origin, cookie
// auth); the server mints the sandbox shell token and dials the provider's
// exec bridge. The provider endpoint and token never reach the browser.
//
// Wire protocol spoken through the relay (pinned against railway-ts-sdk):
// - client → server text frames: init_exec (first)
// - server → client text frames: exit
// - binary frames: first byte tags the stream (0x01 stdout, 0x02 stdin,
//   0x03 stderr), the rest is raw bytes
//
// The bridge has no PTY channel, so the PTY is allocated inside the sandbox
// with `script` and sized once at startup from the rendered terminal.

import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'
import { useEffect, useRef } from 'react'

const WORKSPACE_PATH = '/workspace/app'
const STDOUT = 0x01
const STDIN = 0x02
const STDERR = 0x03

function relayUrl(sessionId: string): string {
  const scheme = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${scheme}//${window.location.host}/api/sessions/${sessionId}/terminal`
}

// Tokyo Night, matching styles.css tokens.
const THEME = {
  background: '#0d0f17',
  foreground: '#c0caf5',
  cursor: '#c0caf5',
  cursorAccent: '#0d0f17',
  selectionBackground: '#3d446780',
  black: '#16161e',
  red: '#f7768e',
  green: '#9ece6a',
  yellow: '#e0af68',
  blue: '#7aa2f7',
  magenta: '#bb9af7',
  cyan: '#7dcfff',
  white: '#a9b1d6',
  brightBlack: '#565f89',
  brightRed: '#f7768e',
  brightGreen: '#9ece6a',
  brightYellow: '#e0af68',
  brightBlue: '#94b8f8',
  brightMagenta: '#bb9af7',
  brightCyan: '#7dcfff',
  brightWhite: '#e8eeff',
}

function shellCommand(workspacePath: string, cols: number, rows: number): string {
  // `script` allocates the PTY; stty sizes it to the rendered terminal.
  return `cd ${workspacePath} 2>/dev/null; exec script -qec 'stty cols ${cols} rows ${rows} 2>/dev/null; exec bash -l' /dev/null`
}

export function SandboxTerminal({
  sessionId,
  onExit,
}: {
  sessionId: string
  onExit?: (message: string) => void
}) {
  const containerRef = useRef<HTMLDivElement>(null)
  const onExitRef = useRef(onExit)
  onExitRef.current = onExit

  useEffect(() => {
    const container = containerRef.current
    if (!container) {
      return
    }

    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: "'Geist Mono', ui-monospace, monospace",
      fontSize: 13,
      theme: THEME,
    })
    const fit = new FitAddon()
    terminal.loadAddon(fit)
    terminal.open(container)
    fit.fit()
    terminal.focus()

    let socket: WebSocket | null = null
    let closed = false
    const encoder = new TextEncoder()

    const finish = (message: string) => {
      if (closed) {
        return
      }
      closed = true
      terminal.write(`\r\n\x1b[90m${message}\x1b[0m\r\n`)
      onExitRef.current?.(message)
    }

    {
      terminal.write('\x1b[90mConnecting to sandbox…\x1b[0m\r\n')
      const ws = new WebSocket(relayUrl(sessionId))
      ws.binaryType = 'arraybuffer'
      socket = ws

      ws.onopen = () => {
        ws.send(
          JSON.stringify({
            type: 'init_exec',
            data: {
              command: shellCommand(WORKSPACE_PATH, terminal.cols, terminal.rows),
            },
          }),
        )
        terminal.write('\x1b[2K\r')
      }
      ws.onmessage = (event) => {
        if (event.data instanceof ArrayBuffer) {
          const bytes = new Uint8Array(event.data)
          if (bytes.length === 0) {
            return
          }
          const tag = bytes[0]
          if (tag === STDOUT || tag === STDERR) {
            terminal.write(bytes.subarray(1))
          }
          return
        }
        if (typeof event.data === 'string') {
          try {
            const frame = JSON.parse(event.data) as { type?: string; data?: { exit_code?: number } }
            if (frame.type === 'exit') {
              finish(`Shell exited (code ${frame.data?.exit_code ?? 0}).`)
              ws.close(1000)
            }
          } catch {
            // Unknown text frames are ignored, matching the CLI's behavior.
          }
        }
      }
      ws.onclose = (event) => {
        finish(event.code === 1000 ? 'Disconnected.' : `Connection closed (${event.code}).`)
      }
      ws.onerror = () => {
        finish('Connection failed.')
      }

      // Disposed with the terminal itself in the effect cleanup.
      terminal.onData((input) => {
        if (ws.readyState === WebSocket.OPEN) {
          const bytes = encoder.encode(input)
          const frame = new Uint8Array(1 + bytes.length)
          frame[0] = STDIN
          frame.set(bytes, 1)
          ws.send(frame)
        }
      })
    }

    const resize = () => fit.fit()
    window.addEventListener('resize', resize)

    return () => {
      closed = true
      window.removeEventListener('resize', resize)
      socket?.close(1000)
      terminal.dispose()
    }
  }, [sessionId])

  return <div ref={containerRef} className="size-full" />
}
