import { createFileRoute, Link } from '@tanstack/react-router';
import {
  AppWindow,
  ChevronDown,
  Droplet,
  Keyboard,
  Palette,
  RotateCcw,
  Search,
  Settings2,
  SwatchBook,
  Terminal,
} from 'lucide-react';
import { ThemeToggle } from '@/components/marketing-page-shell';
import { sponsors } from '@/lib/sponsors';

export const Route = createFileRoute('/')({
  head: () => ({
    links: [
      { rel: 'preconnect', href: 'https://fonts.googleapis.com' },
      {
        rel: 'preconnect',
        href: 'https://fonts.gstatic.com',
        crossOrigin: 'anonymous',
      },
      {
        rel: 'stylesheet',
        href: 'https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;700&display=swap',
      },
    ],
  }),
  component: Home,
});

const MONO = "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, monospace";

/* Deterministic PRNG so SSR and client render the identical sky. */
function mulberry32(seed: number) {
  let a = seed | 0;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

interface Star {
  top: number;
  left: number;
  size: number;
  opacity: number;
  delay: number;
  tint: string;
}

function buildStars(): Star[] {
  const rand = mulberry32(1337);
  const stars: Star[] = [];
  for (let i = 0; i < 160; i++) {
    const bright = rand() > 0.85;
    stars.push({
      top: rand() * 100,
      left: rand() * 100,
      size: bright ? 2.2 : 0.8 + rand() * 1.2,
      opacity: bright ? 0.85 : 0.15 + rand() * 0.5,
      delay: rand() * 8,
      tint: rand() > 0.7 ? '#9fc0ff' : '#c0caf5',
    });
  }
  return stars;
}

const STARS = buildStars();

interface NebulaSegment {
  text: string;
  color?: string;
  opacity?: number;
  token?: boolean;
}

function buildNebula(
  seed: number,
  tokens: Array<[number, number, string]>,
): NebulaSegment[][] {
  const rand = mulberry32(seed);
  const rows = 14;
  const cols = 34;
  const glyphs = '08=≡·:x*ΔΘ○◇B≈°'.split('');
  const palette = ['#6183d6', '#565f89', '#3d59a1', '#7dcfff', '#414868'];

  const chars: Array<
    Array<{
      ch: string;
      color: string;
      opacity: number;
      token?: boolean;
    } | null>
  > = [];
  for (let r = 0; r < rows; r++) {
    const line: Array<{ ch: string; color: string; opacity: number } | null> = [];
    // Gentle diagonal drift so each cloud reads as a streak, not a blob.
    const cx = cols / 2 + (r - rows / 2) * 1.4;
    for (let c = 0; c < cols; c++) {
      const d =
        ((c - cx) / (cols / 2 + 2)) ** 2 + ((r - rows / 2) / (rows / 2 + 1)) ** 2;
      const density = Math.max(0, 0.5 * (1 - d)) + (rand() - 0.5) * 0.08;
      if (rand() < density) {
        line.push({
          ch: glyphs[Math.floor(rand() * glyphs.length)],
          color: palette[Math.floor(rand() * palette.length)],
          opacity:
            Math.round((0.15 + rand() * 0.6 * Math.max(0.2, 1 - d)) * 10) / 10,
        });
      } else {
        line.push(null);
      }
    }
    chars.push(line);
  }

  for (const [row, col, token] of tokens) {
    if (row >= rows) continue;
    for (let i = 0; i < token.length && col + i < cols; i++) {
      chars[row][col + i] = {
        ch: token[i],
        color: '#7aa2f7',
        opacity: 0.9,
        token: true,
      };
    }
  }

  return chars.map((line) => {
    const segments: NebulaSegment[] = [];
    for (const cell of line) {
      const last = segments[segments.length - 1];
      if (cell === null) {
        if (last && last.color === undefined) last.text += ' ';
        else segments.push({ text: ' ' });
      } else if (
        last &&
        last.color === cell.color &&
        last.opacity === cell.opacity &&
        last.token === cell.token
      ) {
        last.text += cell.ch;
      } else {
        segments.push({
          text: cell.ch,
          color: cell.color,
          opacity: cell.opacity,
          token: cell.token,
        });
      }
    }
    return segments;
  });
}

const NEBULA_LEFT = buildNebula(90210, [
  [1, 10, '$ termy'],
  [4, 4, '0.3ns'],
  [7, 14, 'neofetch'],
  [10, 8, 'andies'],
]);

const NEBULA_RIGHT = buildNebula(48151, [
  [1, 12, './termy --fast'],
  [4, 22, 'GPU'],
  [6, 8, 'config'],
  [9, 16, 'rainbow'],
  [11, 10, 'inode'],
]);

function Home() {
  return (
    <main
      className="marketing-theme relative min-h-screen overflow-hidden text-[#c0caf5]"
      style={{
        background:
          'radial-gradient(1100px 520px at 62% 12%, rgba(56, 79, 148, 0.30), transparent 62%), radial-gradient(800px 420px at 20% 30%, rgba(40, 56, 110, 0.20), transparent 60%), radial-gradient(900px 600px at 80% 78%, rgba(34, 48, 96, 0.16), transparent 65%), #0d0f17',
      }}
    >
      <Starfield />

      <div className="relative z-10 flex flex-col items-center">
        <SiteNav />
        <Hero />
        <Showcase />
        <FeatureStrip />
      </div>
    </main>
  );
}

function Starfield() {
  return (
    <div aria-hidden className="pointer-events-none absolute inset-0">
      {STARS.map((star, i) => (
        <span
          key={i}
          className="absolute rounded-full motion-safe:animate-[home-twinkle_6s_ease-in-out_infinite]"
          style={
            {
              top: `${star.top}%`,
              left: `${star.left}%`,
              width: star.size,
              height: star.size,
              background: star.tint,
              opacity: star.opacity,
              animationDelay: `${star.delay}s`,
              boxShadow:
                star.size > 2 ? `0 0 6px 1px ${star.tint}55` : undefined,
              '--star-opacity': star.opacity,
            } as React.CSSProperties
          }
        />
      ))}
      <style>{`
        @keyframes home-twinkle {
          0%, 100% { opacity: var(--star-opacity); }
          50% { opacity: calc(var(--star-opacity) * 0.35); }
        }
      `}</style>
    </div>
  );
}

const navLinkClass =
  'text-sm text-[#a9b1d6] transition-colors hover:text-[#c0caf5]';

function SiteNav() {
  return (
    <header className="flex w-full justify-center px-6 pt-2">
      <div className="flex w-full max-w-6xl items-center justify-between py-5">
        <Link to="/" className="flex min-w-0 flex-1 items-center gap-2.5">
          <span
            className="text-[15px] font-bold text-[#7aa2f7]"
            style={{ fontFamily: MONO }}
          >
            ❯_
          </span>
          <span className="text-[15px] font-medium tracking-tight">termy</span>
        </Link>

        <nav className="hidden items-center gap-8 sm:flex">
          <Link to="/docs/$" params={{ _splat: '' }} className={navLinkClass}>
            Docs
          </Link>
          <Link to="/releases" className={navLinkClass}>
            Releases
          </Link>
          <Link to="/sponsors" className={navLinkClass}>
            Sponsors
          </Link>
          <a
            href="https://github.com/lassejlv/termy"
            target="_blank"
            rel="noreferrer"
            className={navLinkClass}
          >
            GitHub
          </a>
        </nav>

        <div className="flex flex-1 items-center justify-end gap-2">
          <ThemeToggle />
          <Link
            to="/download"
            className="rounded-lg border border-white/[0.12] bg-white/[0.06] px-4 py-2 text-sm font-medium text-[#c0caf5] transition-colors hover:bg-white/[0.1]"
          >
            Download
          </Link>
        </div>
      </div>
    </header>
  );
}

function Hero() {
  return (
    <section className="relative flex w-full flex-col items-center px-6 pt-16 text-center sm:pt-20">
      <NebulaCloud
        segments={NEBULA_LEFT}
        className="left-[max(1rem,calc(50%-46rem))] top-2 -rotate-2"
      />
      <NebulaCloud
        segments={NEBULA_RIGHT}
        className="right-[max(1rem,calc(50%-46rem))] top-10 rotate-2"
      />

      <h1
        className="text-4xl font-medium leading-[1.1] tracking-tight text-balance sm:text-5xl md:text-6xl"
        style={{ fontFamily: MONO }}
      >
        <span className="block text-[#e8eeff]">The terminal,</span>
        <span
          className="block bg-clip-text text-transparent"
          style={{
            backgroundImage:
              'linear-gradient(100deg, #c8d7ff 10%, #7aa2f7 55%, #5a7bd8 95%)',
          }}
        >
          at full speed.
        </span>
      </h1>

      <p className="mt-7 max-w-xl text-lg leading-relaxed text-[#787c99]">
        GPU-accelerated, deeply configurable,
        <br className="hidden sm:block" /> and built for daily work on macOS,
        Windows, and Linux.
      </p>

      <div className="mt-9 flex flex-wrap items-center justify-center gap-7">
        <Link
          to="/download"
          className="rounded-full px-7 py-3.5 text-[15px] font-medium text-[#10192e] shadow-[0_0_35px_rgba(122,162,247,0.4),inset_0_1px_0_rgba(255,255,255,0.7)] transition-transform hover:scale-[1.02] active:scale-[0.98]"
          style={{
            background: 'linear-gradient(180deg, #eaf2ff 0%, #94b8f8 100%)',
          }}
        >
          Download Termy
        </Link>
        <Link
          to="/docs/$"
          params={{ _splat: '' }}
          className="group text-[15px] text-[#c0caf5] transition-colors hover:text-white"
        >
          Read the docs{' '}
          <span
            aria-hidden
            className="inline-block transition-transform group-hover:translate-x-0.5"
          >
            →
          </span>
        </Link>
      </div>
    </section>
  );
}

function NebulaCloud({
  segments,
  className,
}: {
  segments: NebulaSegment[][];
  className: string;
}) {
  return (
    <div
      aria-hidden
      className={`pointer-events-none absolute hidden select-none lg:block ${className}`}
      style={{
        maskImage:
          'radial-gradient(60% 60% at 50% 50%, black 45%, transparent 100%)',
        WebkitMaskImage:
          'radial-gradient(60% 60% at 50% 50%, black 45%, transparent 100%)',
      }}
    >
      <pre
        className="text-[10px] leading-[1.8]"
        style={{ fontFamily: MONO, letterSpacing: '0.42em' }}
      >
        {segments.map((line, row) => {
          const motionSeed = row * 37 + 11;
          const duration = 9 + (motionSeed % 45) / 10;
          const delay = -(motionSeed % 100) / 10;

          return (
            <div
              key={row}
              className="motion-safe:animate-[home-nebula-row_ease-in-out_infinite]"
              style={
                {
                  animationDuration: `${duration}s`,
                  animationDelay: `${delay}s`,
                  '--nebula-x': `${
                    (row % 2 === 0 ? 1 : -1) * (2 + (motionSeed % 3))
                  }px`,
                  '--nebula-y': `${2 + (motionSeed % 4)}px`,
                } as React.CSSProperties
              }
            >
              {line.map((segment, i) => {
                if (!segment.color) return segment.text;
                return (
                  <span
                    key={i}
                    style={{ color: segment.color, opacity: segment.opacity }}
                  >
                    {segment.text}
                  </span>
                );
              })}
            </div>
          );
        })}
      </pre>
      <style>{`
        @keyframes home-nebula-row {
          0%, 100% {
            transform: translate3d(0, 0, 0);
            opacity: 1;
          }
          50% {
            transform: translate3d(var(--nebula-x), calc(var(--nebula-y) * -1), 0);
            opacity: 0.7;
          }
        }
      `}</style>
    </div>
  );
}

const TRAFFIC_LIGHTS = ['#ff5f57', '#febc2e', '#28c840'];

function TrafficLights({ size = 12 }: { size?: number }) {
  return (
    <div className="flex items-center" style={{ gap: size * 0.66 }}>
      {TRAFFIC_LIGHTS.map((color) => (
        <span
          key={color}
          className="rounded-full"
          style={{ width: size, height: size, background: color }}
        />
      ))}
    </div>
  );
}

const WEATHER_ROWS: Array<[string, string, string]> = [
  ['City', 'Aalborg', '#9ece6a'],
  ['Weather', 'Overcast clouds', '#9ece6a'],
  ['Temp', '18.3°C', '#f7768e'],
  ['Wind', '7.6 km/h ↑', '#9ece6a'],
  ['Humidity', '72%', '#9ece6a'],
  ['Precip', '0.0 mm | 0%', '#9ece6a'],
];

function Showcase() {
  return (
    <section className="relative mt-16 w-full max-w-6xl px-4 sm:px-6">
      <div className="relative">
        <div
          className="relative overflow-hidden rounded-t-2xl border border-b-0 border-white/[0.08] bg-[#16161e]/90 shadow-[0_-20px_80px_rgba(61,89,161,0.12)] backdrop-blur-sm"
          style={{ fontFamily: MONO }}
        >
          <div className="flex items-center border-b border-white/[0.05] px-5 py-4">
            <TrafficLights />
          </div>

          <div className="min-h-[430px] px-7 py-6 text-[13.5px] leading-[1.9] sm:text-sm">
            <p>
              <span className="text-[#7aa2f7]">$</span>{' '}
              <span className="text-[#c0caf5]">termy</span>
            </p>
            <p>
              <span className="text-[#7aa2f7]">$</span>{' '}
              <span className="text-[#c0caf5]">neofetch</span>
            </p>

            <div className="mt-4 flex flex-wrap gap-x-10 gap-y-4">
              <pre className="leading-[1.9] text-[#c0caf5]">
                {'    .-.\n'}
                {'   (   ).\n'}
                {'  (___(__)\n'}
                {' (__,_,___)'}
              </pre>
              <div>
                {WEATHER_ROWS.map(([label, value, color]) => (
                  <div key={label} className="flex">
                    <span className="w-28 text-[#7aa2f7]">{label}</span>
                    <span style={{ color }}>{value}</span>
                  </div>
                ))}
              </div>
            </div>

            <p className="mt-8">
              <span className="text-[#7aa2f7]">$</span>{' '}
              <span className="inline-block h-[1.1em] w-[0.55em] translate-y-[0.18em] bg-[#9ece6a] motion-safe:animate-pulse" />
            </p>
          </div>
        </div>

        <SettingsWindow />
      </div>
    </section>
  );
}

const SIDEBAR_ITEMS = [
  { icon: Palette, label: 'Appearance', active: true },
  { icon: Terminal, label: 'Terminal' },
  { icon: AppWindow, label: 'Tabs' },
  { icon: SwatchBook, label: 'Themes' },
  { icon: Droplet, label: 'Colors' },
  { icon: Keyboard, label: 'Keybindings' },
  { icon: Settings2, label: 'Advanced' },
];

function SettingsWindow() {
  return (
    <div
      className="absolute right-6 bottom-6 hidden w-[520px] overflow-hidden rounded-xl border border-white/[0.1] bg-[#1a1b26] shadow-[0_30px_90px_rgba(0,0,0,0.65),0_0_50px_rgba(61,89,161,0.1)] lg:block"
      style={{ fontFamily: MONO }}
    >
      <div className="flex">
        <aside className="flex w-[150px] shrink-0 flex-col border-r border-white/[0.06] bg-[#16161e] p-3">
          <div className="mb-1 px-1 pt-0.5 pb-2">
            <TrafficLights size={8} />
          </div>
          <div className="mb-2 flex items-center gap-1.5 rounded-md border border-white/[0.08] bg-[#1c1f30] px-2 py-1.5 text-[9px] text-[#565f89]">
            <Search className="size-2.5" />
            Search
          </div>
          <nav className="flex flex-col gap-0.5 text-[9.5px]">
            {SIDEBAR_ITEMS.map(({ icon: Icon, label, active }) => (
              <span
                key={label}
                className={`flex items-center gap-1.5 rounded-md px-2 py-1.5 ${
                  active
                    ? 'bg-[#283457] text-[#c0caf5] shadow-[inset_0_0_0_1px_rgba(122,162,247,0.3)]'
                    : 'text-[#787c99]'
                }`}
              >
                <Icon className="size-2.5" />
                {label}
              </span>
            ))}
          </nav>
          <p className="mt-auto px-2 pt-6 text-[8.5px] text-[#565f89]">
            Termy v0.2.6
          </p>
        </aside>

        <div className="flex-1 p-4">
          <div className="flex items-start justify-between">
            <div>
              <h3 className="text-[13px] text-[#c0caf5]">Appearance</h3>
              <p className="mt-0.5 text-[8.5px] text-[#565f89]">
                Customize the look and feel.
              </p>
            </div>
            <span className="rounded-md border border-white/[0.1] px-2 py-1 text-[8.5px] text-[#787c99]">
              Reset section
            </span>
          </div>

          <SettingsSection label="THEME">
            <SettingsRow
              title="Theme Mode"
              description="Use a single theme or switch with system appearance"
            >
              <SelectControl value="Manual (annual)" />
            </SettingsRow>
            <SettingsRow title="Theme" description="Current color scheme name">
              <div className="flex items-center gap-1.5">
                <span className="flex items-center gap-1.5 rounded-md border border-white/[0.1] bg-[#1c1f30] px-2 py-1.5 text-[8.5px] text-[#c0caf5]">
                  tokyo-night-storm-darker
                  <span className="size-2 rounded-[2px] bg-[#7aa2f7]" />
                  <span className="size-2 rounded-[2px] bg-[#414868]" />
                </span>
                <RotateCcw className="size-2.5 text-[#565f89]" />
              </div>
            </SettingsRow>
          </SettingsSection>

          <SettingsSection label="APP">
            <SettingsRow
              title="App Icon"
              description="Manage app icon shown in the Dock and app switcher"
            >
              <SelectControl value="Termy Old" />
            </SettingsRow>
          </SettingsSection>

          <SettingsSection label="CHROME">
            <SettingsRow
              title="Increase Chrome Contrast"
              description="Increase contrast of non-terminal UI surfaces"
            >
              <div className="flex items-center gap-1.5">
                <span className="flex h-3.5 w-6 items-center rounded-full bg-[#7aa2f7] pl-3">
                  <span className="size-2.5 rounded-full bg-white" />
                </span>
                <RotateCcw className="size-2.5 text-[#565f89]" />
              </div>
            </SettingsRow>
          </SettingsSection>
        </div>
      </div>
    </div>
  );
}

function SettingsSection({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="mt-4">
      <p className="text-[8px] tracking-[0.14em] text-[#565f89]">{label}</p>
      <div className="mt-1.5 divide-y divide-white/[0.05] rounded-lg border border-white/[0.06] bg-[#191b28]">
        {children}
      </div>
    </div>
  );
}

function SettingsRow({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-4 px-3 py-2.5">
      <div className="min-w-0">
        <p className="text-[9.5px] text-[#c0caf5]">{title}</p>
        <p className="mt-0.5 text-[8px] leading-snug text-[#565f89]">
          {description}
        </p>
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

function SelectControl({ value }: { value: string }) {
  return (
    <div className="flex items-center gap-1.5">
      <span className="flex items-center gap-3 rounded-md border border-white/[0.1] bg-[#1c1f30] px-2 py-1.5 text-[8.5px] text-[#c0caf5]">
        {value}
        <ChevronDown className="size-2.5 text-[#565f89]" />
      </span>
      <RotateCcw className="size-2.5 text-[#565f89]" />
    </div>
  );
}

const FEATURES: Array<[string, string, string]> = [
  ['platform', 'Native', '#7aa2f7'],
  ['render', 'Fast', '#7aa2f7'],
  ['config', 'Configurable', '#9ece6a'],
];

function FeatureStrip() {
  return (
    <section
      className="relative z-20 w-full border-t border-white/[0.06] bg-[#0d0f17]/80 backdrop-blur-sm"
      style={{ fontFamily: MONO }}
    >
      <div className="mx-auto grid max-w-6xl grid-cols-1 divide-y divide-white/[0.08] sm:grid-cols-3 sm:divide-x sm:divide-y-0">
        {FEATURES.map(([property, label, color]) => (
          <div
            key={label}
            className="flex items-baseline gap-5 px-8 py-7 sm:justify-center lg:gap-8"
          >
            <span className="text-[10px] text-[#565f89]">{property}</span>
            <span className="text-[15px] tracking-wide" style={{ color }}>
              {label}
            </span>
          </div>
        ))}
      </div>
      <div className="border-t border-white/[0.08]">
        <div className="mx-auto grid max-w-6xl md:grid-cols-[12rem_1fr]">
          <div className="flex items-baseline justify-between gap-4 px-8 py-6 md:block md:border-r md:border-white/[0.08]">
            <p className="text-xs text-[#c0caf5]">Supported by</p>
            <p className="mt-1 text-[10px] text-[#565f89]">
              {sponsors.length} {sponsors.length === 1 ? 'supporter' : 'supporters'}
            </p>
            <Link
              to="/sponsors"
              className="mt-2 inline-block text-[10px] text-[#7aa2f7] transition-colors hover:text-white"
            >
              View all →
            </Link>
          </div>
          <div className="grid border-t border-white/[0.08] sm:grid-cols-2 sm:divide-x sm:divide-white/[0.08] md:border-t-0">
            {sponsors.map((sponsor) => (
              <a
                key={sponsor.name}
                href={sponsor.url}
                target="_blank"
                rel="noreferrer"
                className="group flex min-w-0 items-center gap-4 border-t border-white/[0.08] px-8 py-5 transition-colors first:border-t-0 hover:bg-white/[0.04] sm:border-t-0"
              >
                <span
                  className={`flex shrink-0 items-center ${
                    sponsor.avatar ? 'size-9 justify-center' : 'h-9 w-20'
                  }`}
                >
                  <img
                    src={sponsor.logo.light}
                    alt={`${sponsor.name} logo`}
                    loading="lazy"
                    className={`max-h-8 max-w-full object-contain dark:hidden ${
                      sponsor.avatar ? 'rounded-full' : ''
                    }`}
                  />
                  <img
                    src={sponsor.logo.dark}
                    alt={`${sponsor.name} logo`}
                    loading="lazy"
                    className={`hidden max-h-8 max-w-full object-contain dark:block ${
                      sponsor.avatar ? 'rounded-full' : ''
                    }`}
                  />
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block text-xs text-[#c0caf5] transition-colors group-hover:text-white">
                    {sponsor.name}
                  </span>
                  {sponsor.description && (
                    <span className="mt-1 block truncate text-[10px] text-[#565f89]">
                      {sponsor.description}
                    </span>
                  )}
                </span>
                <span
                  aria-hidden
                  className="ml-auto shrink-0 text-[#565f89] transition-[transform,color] group-hover:translate-x-0.5 group-hover:text-[#7aa2f7]"
                >
                  ↗
                </span>
              </a>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}
