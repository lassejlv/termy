import { Link } from '@tanstack/react-router';
import { Moon, Sun } from 'lucide-react';
import { type ReactNode, useEffect, useState } from 'react';

export const marketingFontLinks = [
  { rel: 'preconnect', href: 'https://fonts.googleapis.com' },
  {
    rel: 'preconnect',
    href: 'https://fonts.gstatic.com',
    crossOrigin: 'anonymous' as const,
  },
  {
    rel: 'stylesheet',
    href: 'https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;700&display=swap',
  },
];

export const marketingMono =
  "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, monospace";

function setTheme(theme: 'light' | 'dark') {
  document.documentElement.classList.toggle('dark', theme === 'dark');
  try {
    localStorage.setItem('theme', theme);
  } catch {
    // Theme still applies when storage is unavailable.
  }
}

export function ThemeToggle() {
  const [theme, setCurrentTheme] = useState<'light' | 'dark'>('dark');

  useEffect(() => {
    const root = document.documentElement;
    const syncTheme = () =>
      setCurrentTheme(root.classList.contains('dark') ? 'dark' : 'light');
    const observer = new MutationObserver(syncTheme);

    syncTheme();
    observer.observe(root, { attributes: true, attributeFilter: ['class'] });
    return () => observer.disconnect();
  }, []);

  const nextTheme = theme === 'dark' ? 'light' : 'dark';

  return (
    <button
      type="button"
      aria-label={nextTheme === 'light' ? 'Switch to light theme' : 'Switch to dark theme'}
      onClick={() => {
        setCurrentTheme(nextTheme);
        setTheme(nextTheme);
      }}
      className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-white/[0.04] text-[#c0caf5] transition-colors hover:bg-white/[0.08] active:scale-[0.96]"
    >
      {theme === 'dark' ? (
        <Moon className="size-4" />
      ) : (
        <Sun className="size-4 text-[#f4c76b]" />
      )}
    </button>
  );
}

const navLinkClass =
  'text-sm text-[#a9b1d6] transition-colors hover:text-[#c0caf5]';

function MarketingNav() {
  return (
    <header className="relative z-20 flex w-full justify-center px-6 pt-2">
      <div className="flex w-full max-w-6xl items-center justify-between py-5">
        <Link to="/" className="flex min-w-0 flex-1 items-center gap-2.5">
          <span
            className="text-[15px] font-bold text-[#7aa2f7]"
            style={{ fontFamily: marketingMono }}
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

function PageStars() {
  return (
    <div aria-hidden className="pointer-events-none absolute inset-0 overflow-hidden">
      {Array.from({ length: 72 }, (_, index) => {
        const x = (index * 47 + 13) % 100;
        const y = (index * 71 + 7) % 100;
        const bright = index % 11 === 0;
        return (
          <span
            key={index}
            className="absolute rounded-full bg-[#9fc0ff] motion-safe:animate-[marketing-star_7s_ease-in-out_infinite]"
            style={{
              left: `${x}%`,
              top: `${y}%`,
              width: bright ? 2 : 1,
              height: bright ? 2 : 1,
              opacity: bright ? 0.58 : 0.22,
              animationDelay: `${-(index % 14) / 2}s`,
            }}
          />
        );
      })}
      <style>{`
        @keyframes marketing-star {
          0%, 100% { transform: scale(1); opacity: 0.22; }
          50% { transform: scale(0.65); opacity: 0.5; }
        }
      `}</style>
    </div>
  );
}

export function MarketingPageShell({ children }: { children: ReactNode }) {
  return (
    <div
      className="marketing-theme relative min-h-screen overflow-hidden bg-[#0d0f17] text-[#c0caf5]"
      style={{
        background:
          'radial-gradient(900px 440px at 68% 3%, rgba(56,79,148,0.25), transparent 64%), radial-gradient(700px 480px at 12% 54%, rgba(40,56,110,0.15), transparent 65%), #0d0f17',
      }}
    >
      <PageStars />
      <MarketingNav />
      <div className="relative z-10">{children}</div>
    </div>
  );
}

export const marketingLinkClass =
  'text-[#c0caf5] underline decoration-white/20 underline-offset-4 transition-colors hover:text-white hover:decoration-[#7aa2f7]';

export const marketingPanelClass =
  'overflow-hidden rounded-2xl border border-white/[0.08] bg-[#16161e]/82 shadow-[0_24px_80px_rgba(0,0,0,0.35)] backdrop-blur-sm';
