import { createFileRoute, Link } from '@tanstack/react-router';
import { Minus, Plus } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { SiteShell, containerClass } from '@/components/site-shell';
import { DEMO_COMMANDS, DemoShell } from '@/lib/demo-shell';

// The @termysh/web bundle is vendored into public/web-terminal/ by
// scripts/sync-web-terminal.mjs and loaded at runtime, outside Vite's graph.
const BUNDLE_URL = '/web-terminal/web.js';

type CursorStyle = 'block' | 'bar' | 'underline';

interface WebTerminal {
  readonly ready: Promise<void>;
  readonly cols: number;
  open(parent: HTMLElement): void;
  write(data: string): void;
  clear(): void;
  focus(): void;
  setOptions(options: Record<string, unknown>): void;
  onData(listener: (data: string) => void): { dispose(): void };
  dispose(): void;
}

interface WebTerminalModule {
  Terminal: new (options: Record<string, unknown>) => WebTerminal;
  builtinThemeIds(): string[];
}

export const Route = createFileRoute('/demo')({
  head: () => ({
    meta: [
      { title: 'Live demo · Termy' },
      {
        name: 'description',
        content: "Try Termy's terminal engine in your browser, compiled to WebAssembly.",
      },
    ],
    links: [
      { rel: 'modulepreload', href: BUNDLE_URL },
      { rel: 'preload', href: '/web-terminal/termy.wasm', as: 'fetch', crossOrigin: 'anonymous' as const },
    ],
  }),
  component: DemoPage,
});

const controlClass =
  'rounded-lg border border-rule bg-surface px-3 py-1.5 text-sm text-text transition-colors hover:bg-rule';

function DemoPage() {
  const container = useRef<HTMLDivElement>(null);
  const terminal = useRef<WebTerminal | undefined>(undefined);
  const shell = useRef<DemoShell | undefined>(undefined);
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading');
  const [themes, setThemes] = useState<string[]>([]);
  const [theme, setTheme] = useState('tokyo-night');
  const [fontSize, setFontSize] = useState(14);
  const [cursorStyle, setCursorStyle] = useState<CursorStyle>('block');
  const [cursorBlink, setCursorBlink] = useState(true);

  useEffect(() => {
    let disposed = false;
    let term: WebTerminal | undefined;
    (async () => {
      try {
        const mod = (await import(/* @vite-ignore */ BUNDLE_URL)) as WebTerminalModule;
        if (disposed || !container.current) return;
        term = new mod.Terminal({
          theme: 'tokyo-night',
          fontSize: 14,
          fontFamily: "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, monospace",
          cursorBlink: true,
          padding: 12,
          scrollback: 5000,
          linkHandler: {
            activate: (_event: MouseEvent, uri: string) => window.open(uri, '_blank', 'noopener,noreferrer'),
          },
        });
        term.open(container.current);
        await term.ready;
        if (disposed) return;
        const ids = mod.builtinThemeIds();
        const current = term;
        const demo = new DemoShell({
          write: (data) => current.write(data),
          clear: () => current.write('\x1b[H\x1b[2J\x1b[3J'),
          setTheme: (id) => {
            current.setOptions({ theme: id });
            setTheme(id);
          },
          themes: () => ids,
          get cols() {
            return current.cols;
          },
        });
        term.onData((data) => demo.input(data));
        demo.start();
        terminal.current = term;
        shell.current = demo;
        setThemes(ids);
        setStatus('ready');
        term.focus();
      } catch (error) {
        console.error(error);
        if (!disposed) setStatus('error');
      }
    })();
    return () => {
      disposed = true;
      term?.dispose();
      terminal.current = undefined;
      shell.current = undefined;
    };
  }, []);

  const update = (options: Record<string, unknown>) => {
    terminal.current?.setOptions(options);
    terminal.current?.focus();
  };

  const run = (command: string) => {
    shell.current?.run(command);
    terminal.current?.focus();
  };

  return (
    <SiteShell>
      <section className={`${containerClass} flex flex-col pt-14 sm:pt-24`}>
        <p className="text-sm text-muted">
          @termysh/web · WebAssembly
        </p>
        <h1
          className="mt-3 text-[40px] leading-12 font-medium tracking-[-0.025em] text-fg sm:text-5xl sm:leading-14"
        >
          Termy in your browser
        </h1>
        <p className="mt-4 max-w-[44rem] leading-relaxed text-muted">
          The same Rust terminal engine as the desktop app, compiled to
          WebAssembly and drawn on a canvas. This shell runs entirely in your
          browser. Type <code className="text-fg">help</code>, or try a
          command below.
        </p>

        <div className="mt-8 flex flex-wrap items-center gap-2">
          {DEMO_COMMANDS.map((command) => (
            <button
              key={command}
              type="button"
              disabled={status !== 'ready'}
              onClick={() => run(command)}
              className={` font-mono disabled:opacity-40`}
            >
              {command}
            </button>
          ))}

          <div className="ml-auto flex flex-wrap items-center gap-2">
            <label className="sr-only" htmlFor="demo-theme">
              Theme
            </label>
            <select
              id="demo-theme"
              value={theme}
              disabled={status !== 'ready'}
              onChange={(event) => {
                setTheme(event.target.value);
                update({ theme: event.target.value });
              }}
              className={`${controlClass} bg-surface`}
            >
              {themes.map((id) => (
                <option key={id} value={id}>
                  {id}
                </option>
              ))}
            </select>

            <div className="flex items-center rounded-lg border border-rule bg-surface">
              <button
                type="button"
                aria-label="Smaller text"
                className="px-2 py-1.5 text-text hover:text-fg disabled:opacity-40"
                disabled={fontSize <= 9}
                onClick={() => {
                  const next = fontSize - 1;
                  setFontSize(next);
                  update({ fontSize: next });
                }}
              >
                <Minus className="size-3.5" />
              </button>
              <span className="w-10 text-center text-sm tabular-nums text-muted">{fontSize}px</span>
              <button
                type="button"
                aria-label="Larger text"
                className="px-2 py-1.5 text-text hover:text-fg disabled:opacity-40"
                disabled={fontSize >= 24}
                onClick={() => {
                  const next = fontSize + 1;
                  setFontSize(next);
                  update({ fontSize: next });
                }}
              >
                <Plus className="size-3.5" />
              </button>
            </div>

            <div className="flex rounded-lg border border-rule bg-surface p-0.5" role="radiogroup" aria-label="Cursor style">
              {(['block', 'bar', 'underline'] as const).map((style) => (
                <button
                  key={style}
                  type="button"
                  role="radio"
                  aria-checked={cursorStyle === style}
                  onClick={() => {
                    setCursorStyle(style);
                    update({ cursorStyle: style });
                  }}
                  className={`rounded-md px-2.5 py-1 text-sm transition-colors ${
                    cursorStyle === style ? 'bg-paper text-fg' : 'text-muted hover:text-text'
                  }`}
                >
                  {style}
                </button>
              ))}
            </div>

            <button
              type="button"
              aria-pressed={cursorBlink}
              onClick={() => {
                setCursorBlink(!cursorBlink);
                update({ cursorBlink: !cursorBlink });
              }}
              className={controlClass}
            >
              blink {cursorBlink ? 'on' : 'off'}
            </button>
          </div>
        </div>

        <div className={`overflow-hidden rounded-xl bg-term-bg shadow-[0_0_0_1px_rgba(0,0,0,0.9),0_30px_60px_-20px_rgba(0,0,0,0.35)] relative mt-4 h-[min(560px,65vh)] min-h-[320px]`}>
          <div ref={container} className="absolute inset-0" />
          {status !== 'ready' && (
            <div
              className="absolute inset-0 flex items-center justify-center text-sm text-muted"
            >
              {status === 'loading'
                ? 'Loading WebAssembly engine…'
                : 'The terminal could not load. Your browser needs WebAssembly and Canvas support.'}
            </div>
          )}
        </div>

        <section className="mt-12 grid gap-8 md:grid-cols-2">
          <div>
            <h2 className="text-lg font-medium text-fg">
              Use it in your app
            </h2>
            <pre
              className="font-mono mt-4 overflow-x-auto rounded-xl border border-rule bg-surface p-4 text-[13px] leading-relaxed text-muted"
            >
              {`npm install @termysh/web

import { Terminal } from '@termysh/web'

const term = new Terminal({ theme: 'tokyo-night' })
term.open(element)
term.onData((data) => socket.send(data))
socket.onmessage = (e) => term.write(e.data)`}
            </pre>
          </div>
          <div>
            <h2 className="text-lg font-medium text-fg">
              What you are looking at
            </h2>
            <ul className="mt-4 space-y-2 leading-relaxed text-muted">
              <li>Parsing, scrollback and key encoding from Termy's Rust engine</li>
              <li>Kitty keyboard and graphics protocols, OSC 8 links, truecolor</li>
              <li>Pixel-exact box drawing, blocks and Braille</li>
              <li>14 bundled themes and live options</li>
              <li>
                An xterm.js-compatible API in <code className="text-text">@termysh/xterm</code>
              </li>
            </ul>
            <p className="mt-5">
              <Link to="/docs/$" params={{ _splat: 'developer/web' }} className="site-link text-fg">
                Read the web terminal docs
              </Link>
            </p>
          </div>
        </section>
      </section>
    </SiteShell>
  );
}
