import { Link, useLocation } from '@tanstack/react-router';
import { Moon, Sun } from 'lucide-react';
import { type CSSProperties, type ReactNode, useEffect, useState } from 'react';
import { gitConfig } from '@/lib/shared';
import { sponsors } from '@/lib/sponsors';

export const repoUrl = `https://github.com/${gitConfig.user}/${gitConfig.repo}`;

/** Page edges: 120px at 1440, a 1200px column, narrower edges below that. */
export const containerClass = 'mx-auto w-full max-w-[1280px] px-5 sm:px-10';

export function TermyMark({ size = 28 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 256 256" aria-hidden className="shrink-0">
      <rect x="8" y="8" width="240" height="240" rx="56" style={{ fill: 'var(--text)' }} />
      <path
        d="M76 84 L124 128 L76 172"
        fill="none"
        strokeWidth="20"
        strokeLinecap="round"
        strokeLinejoin="round"
        style={{ stroke: 'var(--paper)' }}
      />
      <rect x="140" y="162" width="48" height="16" rx="8" style={{ fill: 'var(--paper)' }} />
    </svg>
  );
}

export function AppleIcon() {
  return (
    <svg width="15" height="18" viewBox="0 0 384 512" aria-hidden className="shrink-0">
      <path
        fill="currentColor"
        d="M318.7 268.7c-.2-36.7 16.4-64.4 50-84.8-18.8-26.9-47.2-41.7-84.7-44.6-35.5-2.8-74.3 20.7-88.5 20.7-15 0-49.4-19.7-76.4-19.7C63.3 141.2 4 184.8 4 273.5q0 39.3 14.4 81.2c12.8 36.7 59 126.7 107.2 125.2 25.2-.6 43-17.9 75.8-17.9 31.8 0 48.3 17.9 76.4 17.9 48.6-.7 90.4-82.5 102.6-119.3-65.2-30.7-61.7-90-61.7-91.9zm-56.6-164.2c27.3-32.4 24.8-61.9 24-72.5-24.1 1.4-52 16.4-67.9 34.9-17.5 19.8-27.8 44.3-25.6 71.9 26.1 2 49.9-11.4 69.5-34.3z"
      />
    </svg>
  );
}

/** The one primary action style: a flat near-black fill (off-white on dark). */
export const primaryButtonClass =
  'inline-flex items-center gap-2.5 rounded-[10px] bg-text px-5 py-3.5 text-base leading-5 font-medium text-on-text transition-[background-color,transform] duration-150 ease-out hover:bg-text/90 active:scale-[0.98]';

function setTheme(theme: 'light' | 'dark') {
  document.documentElement.classList.remove('light', 'dark');
  document.documentElement.classList.add(theme);
  document.documentElement.style.colorScheme = theme;
  try {
    localStorage.setItem('theme', theme);
  } catch {
    // The theme still applies when storage is unavailable.
  }
}

export function ThemeToggle() {
  const [theme, setCurrentTheme] = useState<'light' | 'dark'>('light');

  useEffect(() => {
    const root = document.documentElement;
    const sync = () => setCurrentTheme(root.classList.contains('dark') ? 'dark' : 'light');
    const observer = new MutationObserver(sync);
    sync();
    observer.observe(root, { attributes: true, attributeFilter: ['class'] });
    return () => observer.disconnect();
  }, []);

  const next = theme === 'dark' ? 'light' : 'dark';

  return (
    <button
      type="button"
      aria-label={next === 'light' ? 'Switch to light theme' : 'Switch to dark theme'}
      onClick={() => {
        setCurrentTheme(next);
        setTheme(next);
      }}
      className="-mr-2 flex size-8 shrink-0 items-center justify-center rounded-md text-muted transition-colors hover:text-fg"
    >
      {theme === 'dark' ? <Moon className="size-4" /> : <Sun className="size-4" />}
    </button>
  );
}

function SiteLink({ section, children, className }: { section: 'download' | 'releases' | 'docs'; children: ReactNode; className?: string }) {
  if (section === 'docs') {
    return (
      <Link to="/docs/$" params={{ _splat: '' }} className={className}>
        {children}
      </Link>
    );
  }
  return (
    <Link to={section === 'download' ? '/download' : '/releases'} className={className}>
      {children}
    </Link>
  );
}

const navItems = [
  { label: 'Download', section: 'download' },
  { label: 'Releases', section: 'releases' },
  { label: 'Docs', section: 'docs' },
] as const;

export function SiteNav({ bordered = false }: { bordered?: boolean }) {
  const pathname = useLocation({ select: (location) => location.pathname });

  return (
    <header className={bordered ? 'border-b border-rule' : undefined}>
      <div className={`${containerClass} flex items-center justify-between gap-4 py-5 sm:py-7`}>
        <Link to="/" className="flex items-center gap-2.5" aria-label="Termy home">
          <TermyMark />
          <span className="text-xl leading-6 font-medium tracking-[-0.01em] text-fg">Termy</span>
        </Link>
        <nav className="flex items-center gap-3.5 text-sm leading-5 sm:gap-8 sm:text-[15px]">
          {navItems.map((item) => {
            const base = `/${item.section}`;
            const current = pathname === base || pathname.startsWith(`${base}/`);
            return (
              <SiteLink
                key={item.section}
                section={item.section}
                className={current ? 'text-text' : 'text-muted transition-colors hover:text-text'}
              >
                {item.label}
              </SiteLink>
            );
          })}
          <a
            href={repoUrl}
            target="_blank"
            rel="noreferrer"
            className="hidden text-muted transition-colors hover:text-text sm:inline"
          >
            GitHub
          </a>
          <ThemeToggle />
        </nav>
      </div>
    </header>
  );
}

const sponsorLine = `Supported by ${new Intl.ListFormat('en', { type: 'conjunction' }).format(
  sponsors.map((sponsor) => sponsor.name),
)}.`;

export function SiteFooter() {
  return (
    <footer className={`${containerClass} pt-24 pb-14 sm:pt-30`}>
      <div className="flex flex-col gap-6 border-t border-rule pt-7 md:flex-row md:items-center md:justify-between">
        <Link to="/sponsors" className="flex items-center gap-3 text-[15px] leading-5 text-fg">
          <TermyMark size={22} />
          {sponsorLine}
        </Link>
        <nav className="flex flex-wrap gap-x-7 gap-y-2 text-[15px] leading-5 text-muted">
          {navItems.map((item) => (
            <SiteLink key={item.section} section={item.section} className="transition-colors hover:text-text">
              {item.label}
            </SiteLink>
          ))}
          <Link to="/sponsors" className="transition-colors hover:text-text">
            Sponsors
          </Link>
          <a href={repoUrl} target="_blank" rel="noreferrer" className="transition-colors hover:text-text">
            GitHub
          </a>
        </nav>
      </div>
      <p className="pt-5 text-[13px] leading-[18px] text-muted">MIT licensed. Built in Rust.</p>
    </footer>
  );
}

export function SiteShell({ children }: { children: ReactNode }) {
  return (
    <div className="flex min-h-screen flex-col bg-paper text-text">
      <SiteNav />
      <main className="flex-1">{children}</main>
      <SiteFooter />
    </div>
  );
}

type ArrowKind = 'left' | 'down' | 'down-left';

const arrows: Record<ArrowKind, { width: number; height: number; paths: string[] }> = {
  left: {
    width: 64,
    height: 30,
    paths: ['M60 18 C 44.8 8, 22.4 10, 6 15', 'M15 8 L5 15 L15 21'],
  },
  down: {
    width: 60,
    height: 150,
    paths: ['M36 4 C 20 30, 14 80, 24 140', 'M16 130 L24 141 L31 129'],
  },
  'down-left': {
    width: 48,
    height: 44,
    paths: ['M44 8 C 28 8, 12 16, 8 38', 'M2 30 L8 39 L14 30'],
  },
};

/** A hand-drawn ink arrow; only drawn where the margin has room for it. */
export function NoteArrow({
  kind,
  className = '',
  style,
}: {
  kind: ArrowKind;
  className?: string;
  style?: CSSProperties;
}) {
  const arrow = arrows[kind];
  return (
    <svg
      width={arrow.width}
      height={arrow.height}
      viewBox={`0 0 ${arrow.width} ${arrow.height}`}
      aria-hidden
      className={`pointer-events-none absolute hidden min-[1180px]:block ${className}`}
      style={style}
    >
      {arrow.paths.map((d) => (
        <path
          key={d}
          d={d}
          fill="none"
          strokeWidth="1.6"
          strokeLinecap="round"
          strokeLinejoin="round"
          style={{ stroke: 'var(--ink)' }}
        />
      ))}
    </svg>
  );
}

export const handClass =
  'font-hand text-[26px] leading-7 whitespace-pre-line text-ink origin-top-left -rotate-2';

/**
 * A handwritten note in the right margin, pointing back at the element it
 * follows. Put it inside a relative wrapper the width of the reading column.
 * Below 1180px there is no margin, so the note sits under that element
 * without its arrow.
 */
export function MarginNote({
  children,
  top = 0,
  arrowGap = 18,
  textGap = 88,
}: {
  children: ReactNode;
  top?: number;
  arrowGap?: number;
  textGap?: number;
}) {
  return (
    <>
      <NoteArrow kind="left" style={{ left: `calc(100% + ${arrowGap}px)`, top: top + 2 }} />
      <p
        className={`${handClass} mt-3 min-[1180px]:absolute min-[1180px]:top-(--note-top) min-[1180px]:left-(--note-left) min-[1180px]:mt-0 min-[1180px]:w-max`}
        style={{ '--note-left': `calc(100% + ${textGap}px)`, '--note-top': `${top}px` } as CSSProperties}
      >
        {children}
      </p>
    </>
  );
}
