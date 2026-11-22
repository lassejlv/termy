import { createFileRoute, Link } from '@tanstack/react-router';
import { type CSSProperties, useEffect, useRef, useState } from 'react';
import {
  AppleIcon,
  MarginNote,
  NoteArrow,
  SiteShell,
  containerClass,
  handClass,
  primaryButtonClass,
} from '@/components/site-shell';
import { fetchLatestGitHubRelease, releaseVersion } from '@/lib/github-release';

export const Route = createFileRoute('/')({
  component: Home,
});

function Home() {
  return (
    <SiteShell>
      <Hero />
      <Speed />
      <Config />
      <Closing />
    </SiteShell>
  );
}

function Hero() {
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchLatestGitHubRelease().then(
      (release) => {
        if (!cancelled) setVersion(releaseVersion(release.tagName));
      },
      (error: unknown) => console.error('Failed to load latest release', error),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <section className={containerClass}>
      <div className="flex flex-col items-start gap-5 pt-14 sm:gap-6 sm:pt-24">
        <h1 className="text-[44px] leading-[48px] font-medium tracking-[-0.03em] text-fg sm:text-[72px] sm:leading-[76px]">
          The terminal, at full speed.
        </h1>
        <p className="max-w-[860px] text-[17px] leading-[26px] text-muted sm:text-[19px] sm:leading-7">
          GPU-accelerated, deeply configurable, and built for daily work on macOS, Windows, and Linux.
        </p>
        <div className="flex flex-wrap items-center gap-x-7 gap-y-4 pt-3 text-[15px] leading-5">
          <Link to="/download" className={primaryButtonClass}>
            <AppleIcon />
            Download for macOS
          </Link>
          <Link to="/download" hash="linux" className="site-link text-text">
            Linux and Windows
          </Link>
          <span className="text-muted">{version ? `${version}, MIT licensed` : 'MIT licensed'}</span>
        </div>
      </div>
      <div className="relative pt-10 pb-4 sm:pt-12 sm:pb-[42px]">
        <p
          className={`${handClass} absolute hidden min-[1180px]:block`}
          style={{ left: '80.67%', top: -108, rotate: '-3deg' }}
        >
          {"The pane you're not in\nsteps back a little."}
        </p>
        <NoteArrow kind="down" style={{ left: '78.33%', top: -56 }} />
        <TerminalWindow />
      </div>
    </section>
  );
}

const prompt = (dir: string) => (
  <>
    <span className="text-term-dim">{dir}</span> <span className="text-term-dim">main</span>
    {'\n'}
    <span className="text-term-green">❯</span>{' '}
  </>
);

function TerminalWindow() {
  return (
    <div className="hero-window flex h-[340px] flex-col overflow-hidden rounded-xl bg-term-bg shadow-[0_0_0_1px_rgba(0,0,0,0.9),0_30px_60px_-20px_rgba(0,0,0,0.35),0_12px_24px_-12px_rgba(0,0,0,0.25)] sm:h-[470px]">
      <div className="flex h-11 shrink-0 items-center gap-4 border-b border-term-line bg-term-surface px-3.5">
        <div className="flex gap-2 pr-1.5" aria-hidden>
          <span className="size-3 rounded-full bg-[#ff5f57]" />
          <span className="size-3 rounded-full bg-[#febc2e]" />
          <span className="size-3 rounded-full bg-[#28c840]" />
        </div>
        <div className="flex gap-1 text-[13px] leading-4">
          <span className="flex h-[30px] items-center rounded-[7px] bg-[#262626] px-[18px] text-term-text">
            zsh ~/Dev/termy
          </span>
          <span className="hidden h-[30px] items-center px-[18px] text-term-dim sm:flex">nvim ~/notes</span>
        </div>
      </div>
      <div className="flex min-h-0 flex-1 font-mono text-[11px] leading-[19px] whitespace-pre text-term-muted sm:text-[13px] sm:leading-[22px]">
        <pre className="min-w-0 flex-1 overflow-hidden px-5 py-[18px] font-[inherit] md:w-[58.33%] md:flex-none md:border-r md:border-term-line">
          {prompt('~/Dev/termy')}
          <span className="text-term-text">git log --oneline -4</span>
          {'\n'}
          <span className="text-term-dim">8d47445a</span> chore: prepare Termy 1.0.0-rc.12 and Termy Next 0.5.0{'\n'}
          <span className="text-term-dim">146b6041</span> fix(next): detect Antigravity CLI and show its logo in tabs
          {'\n'}
          <span className="text-term-dim">17636adb</span> Merge pull request #469 from lassejlv/perf/next-reduce-cpu-usage
          {'\n'}
          <span className="text-term-dim">44dd5e4b</span> perf(next): reduce idle and active CPU usage{'\n\n'}
          {prompt('~/Dev/termy')}
          <span className="text-term-text">cat ~/.config/termy/config.txt</span>
          {'\n'}theme = termy{'\n'}font_family = JetBrains Mono{'\n'}font_size = 14{'\n'}keybind =
          cmd-p=toggle_command_palette{'\n\n'}
          {prompt('~/Dev/termy')}
          <span className="terminal-cursor inline-block h-[13px] w-[7px] translate-y-[2px] bg-term-text sm:h-[17px] sm:w-2 sm:translate-y-[3px]" />
        </pre>
        <pre aria-hidden className="hidden min-w-0 flex-1 overflow-hidden px-5 py-[18px] font-[inherit] opacity-50 md:block">
          {prompt('~/Dev/termy/crates')}
          <span className="text-term-text">ls</span>
          {'\n'}cli{'  '}core{'  '}desktop_app{'  '}engine_ffi{'  '}pty{'  '}wasm{'\n\n'}
          {prompt('~/Dev/termy/crates')}
          <span className="text-term-text">ls ../docs/releases</span>
          {'\n'}1.0.0-rc.10.md{'  '}1.0.0-rc.12.md{'  '}1.0.0-rc.9.md{'\n'}1.0.0-rc.11.md{'  '}1.0.0-rc.8.md
          {'\n\n'}
          {prompt('~/Dev/termy/crates')}
        </pre>
      </div>
    </div>
  );
}

/**
 * Median feed throughput in MiB/s from the engine benchmark of 5 October 2026
 * (docs/engineering/custom-engine-throughput-2026-10-05.md, first table).
 * Bars are drawn to scale in /art/throughput-*.png, one 20px strip per row.
 */
const throughput = [
  { workload: 'Plain scrolling', alacritty: 71.43, termy: 169.635, ratio: 2.428 },
  { workload: 'Styled redraw', alacritty: 47.423, termy: 90.438, ratio: 1.887 },
  { workload: 'Mixed Unicode', alacritty: 68.966, termy: 92.939, ratio: 1.36 },
  { workload: '16,384 distinct Unicode scalars', alacritty: 63.189, termy: 110.355, ratio: 1.787 },
  { workload: 'Repeated combining marks', alacritty: 39.122, termy: 60.747, ratio: 1.551 },
  { workload: '112 distinct combining marks', alacritty: 31.132, termy: 62.401, ratio: 2.032 },
  { workload: 'One-byte plain-text fragments', alacritty: 1.78, termy: 8.55, ratio: 4.804 },
];
const fastest = Math.max(...throughput.map((row) => row.termy));
const share = (value: number) => `${((value / fastest) * 100).toFixed(2)}%`;

const rowGrid =
  'grid grid-cols-[minmax(0,1fr)_auto] gap-x-4 gap-y-3 py-4 [grid-template-areas:"label_ratio""bar_bar"] min-[1180px]:h-16 min-[1180px]:grid-cols-[340px_minmax(0,1fr)_140px] min-[1180px]:items-center min-[1180px]:gap-0 min-[1180px]:py-0 min-[1180px]:[grid-template-areas:"label_bar_ratio"]';

function Speed() {
  const chart = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const node = chart.current;
    if (!node || window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (!entry.isIntersecting) return;
        node.dataset.reveal = 'shown';
        observer.disconnect();
      },
      { threshold: 0.35 },
    );
    const bounds = node.getBoundingClientRect();
    if (bounds.top > window.innerHeight) node.dataset.reveal = 'pending';
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  return (
    <section className={`${containerClass} flex flex-col gap-4 pt-24 sm:pt-30`}>
      <p className="text-[15px] leading-5 text-muted">Speed</p>
      <h2 className="text-[26px] leading-8 font-medium tracking-[-0.015em] text-fg sm:text-[32px] sm:leading-10">
        Faster than Alacritty in all seven workloads we measured.
      </h2>
      <div ref={chart} className="speed-chart relative pt-6 min-[1180px]:pt-10">
        <p className={`${handClass} flex items-center gap-3 pb-2 min-[1180px]:hidden`}>
          <span aria-hidden className="h-6 w-0.5 bg-ink" />
          Alacritty, same machine.
        </p>
        <div
          aria-hidden
          className="pointer-events-none absolute inset-y-0 right-[140px] left-[340px] hidden min-[1180px]:block"
        >
          <p className={`${handClass} absolute`} style={{ left: '48.06%', top: 0 }}>
            Alacritty, same machine.
          </p>
          <NoteArrow kind="down-left" style={{ left: '41.39%', top: 10 }} />
          <p className={`${handClass} absolute`} style={{ left: '15.28%', top: 438 }}>
            Slowest case. Still 4.8×.
          </p>
          <NoteArrow kind="left" style={{ left: '5.56%', top: 440 }} />
        </div>
        <table className="w-full">
          <caption className="sr-only">
            Median feed throughput in MiB/s, Termy's engine against Alacritty's
          </caption>
          <tbody className="block">
            {throughput.map((row, index) => (
              <tr key={row.workload} className={`${rowGrid} border-b border-dotted border-rule`}>
                <th
                  scope="row"
                  className="text-left text-[17px] leading-6 font-normal text-fg [grid-area:label]"
                >
                  {row.workload}
                </th>
                <td className="relative aspect-[720/36] max-w-[720px] [grid-area:bar] min-[1180px]:h-9">
                  <span className="sr-only">
                    {row.termy} MiB/s against {row.alacritty} MiB/s
                  </span>
                  <span
                    aria-hidden
                    className="throughput-bar absolute inset-x-0 top-[22.22%] h-[55.56%]"
                    style={{ '--row': index } as CSSProperties}
                  />
                  <span
                    aria-hidden
                    className="absolute top-0 h-full w-0.5 bg-ink"
                    style={{ left: `calc(${share(row.alacritty)} - 1px)` }}
                  />
                </td>
                <td className="text-right text-[17px] leading-6 text-fg tabular-nums [grid-area:ratio]">
                  {row.ratio.toFixed(1)}×
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        <p className={`${handClass} pt-3 min-[1180px]:hidden`}>Slowest case. Still 4.8×.</p>
      </div>
      <p className="pt-2 text-[13px] leading-[18px] text-muted">
        Median feed throughput, Termy's engine against Alacritty's on the same Linux x86_64 machine. Engine
        benchmark, 5 October 2026.
      </p>
    </section>
  );
}

const configLines = [
  ['Theme', 'theme = termy'],
  ['Font', 'font_family = JetBrains Mono'],
  ['Keys', 'keybind = cmd-p=toggle_command_palette'],
  ['Tasks', 'task.build.command = cargo build'],
  ['Window', 'background_opacity = 0.88'],
];

function Config() {
  return (
    <section className={`${containerClass} flex flex-col gap-4 pt-28 sm:pt-40`}>
      <p className="text-[15px] leading-5 text-muted">Config</p>
      <h2 className="text-[26px] leading-8 font-medium tracking-[-0.015em] text-fg sm:text-[32px] sm:leading-10">
        Shape it in one plain text file.
      </h2>
      <dl className="flex w-full max-w-[760px] flex-col pt-6 sm:pt-8">
        {configLines.map(([label, line], index) => (
          <div key={label} className="relative">
            <div className="flex flex-col gap-1 py-3 sm:h-13 sm:flex-row sm:items-center sm:gap-4 sm:py-0">
              <dt className="text-[17px] leading-6 text-fg">{label}</dt>
              <span aria-hidden className="leader hidden sm:block" />
              <dd className="font-mono text-[13px] leading-5 text-muted sm:text-sm">{line}</dd>
            </div>
            {index === 0 && (
              <MarginNote top={0} arrowGap={10} textGap={80}>
                {'818 themes built in,\nno download needed.'}
              </MarginNote>
            )}
          </div>
        ))}
      </dl>
      <div className="flex flex-wrap items-center gap-x-6 gap-y-3 pt-5 text-[15px] leading-5 sm:pt-7">
        <code className="font-mono text-sm text-fg">~/.config/termy/config.txt</code>
        <Link
          to="/docs/$"
          params={{ _splat: 'reference/configuration-reference' }}
          className="site-link text-fg"
        >
          Configuration reference
        </Link>
      </div>
    </section>
  );
}

function Closing() {
  return (
    <section className={`${containerClass} flex flex-col gap-7 pt-28 sm:pt-40`}>
      <h2 className="text-[36px] leading-[42px] font-medium tracking-[-0.025em] text-fg sm:text-[48px] sm:leading-[56px]">
        Try it with your own shell.
      </h2>
      <div className="flex flex-wrap items-center gap-x-7 gap-y-4 text-[15px] leading-5 text-fg">
        <Link to="/download" className={primaryButtonClass}>
          <AppleIcon />
          Download for macOS
        </Link>
        <Link to="/download" hash="linux" className="site-link">
          Linux
        </Link>
        <Link to="/download" hash="windows" className="site-link">
          Windows
        </Link>
        <Link to="/download" hash="source" className="site-link">
          Build from source
        </Link>
      </div>
    </section>
  );
}
