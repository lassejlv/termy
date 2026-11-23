import { createFileRoute, Link } from '@tanstack/react-router';
import { useEffect, useState } from 'react';
import { MarginNote, SiteShell, containerClass, repoUrl } from '@/components/site-shell';
import {
  fetchGitHubReleases,
  formatLongDate,
  releaseHighlights,
  releaseVersion,
  type GitHubRelease,
  type ReleaseLine,
} from '@/lib/github-release';

export const Route = createFileRoute('/releases/')({
  component: ReleasesPage,
});

type ReleasesState =
  | { status: 'loading' }
  | { status: 'loaded'; releases: GitHubRelease[] }
  | { status: 'error' };

const FEATURED = 6;
const EARLIER_PREVIEW = 6;

const lines: Record<ReleaseLine, { title: string; intro: string }> = {
  termy: { title: 'Termy', intro: 'Release candidates for Termy 1.0, newest first.' },
  next: { title: 'Termy Next', intro: 'The native Swift terminal built on libtermy, newest first.' },
};

function ReleasesPage() {
  // The page is prerendered once, so the line is read from `?line=next`
  // after hydration rather than during render.
  const [line, setLine] = useState<ReleaseLine>('termy');
  const [state, setState] = useState<ReleasesState>({ status: 'loading' });
  const [showAllEarlier, setShowAllEarlier] = useState(false);

  useEffect(() => {
    if (new URLSearchParams(window.location.search).get('line') === 'next') setLine('next');
  }, []);

  const selectLine = (next: ReleaseLine) => {
    setLine(next);
    setShowAllEarlier(false);
    const url = new URL(window.location.href);
    if (next === 'next') url.searchParams.set('line', 'next');
    else url.searchParams.delete('line');
    window.history.replaceState(window.history.state, '', url);
  };

  useEffect(() => {
    let cancelled = false;
    fetchGitHubReleases().then(
      (releases) => {
        if (!cancelled) setState({ status: 'loaded', releases });
      },
      (error: unknown) => {
        console.error('Failed to load releases', error);
        if (!cancelled) setState({ status: 'error' });
      },
    );
    return () => {
      cancelled = true;
    };
  }, []);

  const releases = state.status === 'loaded' ? state.releases.filter((release) => release.line === line) : [];
  const latestStableId = releases.find((release) => !release.prerelease)?.id;
  const featured = releases.slice(0, FEATURED);
  const earlier = releases.slice(FEATURED);
  const visibleEarlier = showAllEarlier ? earlier : earlier.slice(0, EARLIER_PREVIEW);

  return (
    <SiteShell>
      <section className={`${containerClass} flex flex-col items-start gap-5 pt-14 sm:pt-24`}>
        <h1 className="text-[40px] leading-12 font-medium tracking-[-0.025em] text-fg sm:text-5xl sm:leading-14">
          Releases
        </h1>
        <p className="text-[17px] leading-6 text-muted">{lines[line].intro}</p>
        <nav aria-label="Release line" className="flex gap-6 pt-5 text-[15px] leading-5">
          {(['termy', 'next'] as const).map((option) => (
            <button
              key={option}
              type="button"
              onClick={() => selectLine(option)}
              aria-pressed={option === line}
              className={
                option === line
                  ? 'text-text underline decoration-1 underline-offset-[6px]'
                  : 'text-muted transition-colors hover:text-text'
              }
            >
              {lines[option].title}
            </button>
          ))}
        </nav>
      </section>

      <section className={`${containerClass} flex flex-col gap-14 pt-14 sm:gap-16 sm:pt-18`}>
        {state.status === 'loading' && <p className="text-[17px] leading-6 text-muted">Loading releases…</p>}
        {state.status === 'error' && (
          <p className="text-[17px] leading-6 text-muted">
            Could not load releases.{' '}
            <a href={`${repoUrl}/releases`} className="site-link text-fg">
              View them on GitHub
            </a>
          </p>
        )}
        {state.status === 'loaded' && releases.length === 0 && (
          <p className="text-[17px] leading-6 text-muted">No releases yet.</p>
        )}

        {featured.map((release) => (
          <article key={release.id} className="relative flex w-full max-w-[760px] flex-col gap-3.5">
            <header className="flex flex-wrap items-center gap-x-4 gap-y-1 sm:h-8 sm:flex-nowrap">
              <h2 className="text-[21px] leading-7 font-medium tracking-[-0.01em] text-fg">
                <Link to="/releases/$slug" params={{ slug: release.tagName }} className="hover:underline hover:decoration-1 hover:underline-offset-[6px]">
                  {releaseVersion(release.tagName)}
                </Link>
              </h2>
              <ReleaseStatus release={release} latestStableId={latestStableId} />
              <span aria-hidden className="leader hidden sm:block" />
              <time dateTime={release.publishedAt} className="w-full text-[15px] leading-5 text-muted sm:w-auto">
                {formatLongDate(release.publishedAt)}
              </time>
            </header>
            <ul className="flex flex-col gap-1.5">
              {releaseHighlights(release).map((highlight) => (
                <li key={highlight} className="text-base leading-[26px] text-fg">
                  <InlineCode text={highlight} />
                </li>
              ))}
            </ul>
            {release.id === latestStableId && (
              <MarginNote top={-6}>{'The one the Download\nbutton gives you.'}</MarginNote>
            )}
          </article>
        ))}

        {earlier.length > 0 && (
          <div className="flex w-full max-w-[760px] flex-col gap-3.5">
            <h2 className="text-[15px] leading-5 font-normal text-muted">Earlier</h2>
            {visibleEarlier.map((release) => (
              <div key={release.id} className="relative">
                <Link
                  to="/releases/$slug"
                  params={{ slug: release.tagName }}
                  className="group flex h-8 items-center gap-4"
                >
                  <span className="text-[17px] leading-6 text-fg group-hover:underline group-hover:decoration-1 group-hover:underline-offset-[6px]">
                    {releaseVersion(release.tagName)}
                  </span>
                  <span aria-hidden className="leader" />
                  <time dateTime={release.publishedAt} className="text-[15px] leading-5 text-muted">
                    {formatLongDate(release.publishedAt)}
                  </time>
                </Link>
                {release.tagName === 'v1.0.0-rc.1' && (
                  <MarginNote top={-4}>{'Its own engine.\nBye, Alacritty.'}</MarginNote>
                )}
              </div>
            ))}
            <div className="flex flex-wrap gap-x-7 gap-y-3 pt-1 text-[15px] leading-5 text-fg">
              {earlier.length > visibleEarlier.length && (
                <button type="button" onClick={() => setShowAllEarlier(true)} className="site-link">
                  Show {earlier.length - visibleEarlier.length} more
                </button>
              )}
              <a href={`${repoUrl}/releases`} target="_blank" rel="noreferrer" className="site-link">
                Every release on GitHub
              </a>
            </div>
          </div>
        )}
      </section>
    </SiteShell>
  );
}

function ReleaseStatus({ release, latestStableId }: { release: GitHubRelease; latestStableId?: number }) {
  if (release.prerelease) return <span className="text-[15px] leading-5 text-muted">Pre-release</span>;
  if (release.id === latestStableId) return <span className="text-[15px] leading-5 text-muted">Latest</span>;
  return null;
}

/** Release notes mark code with backticks; set those spans in mono. */
function InlineCode({ text }: { text: string }) {
  return text.split(/(`[^`]+`)/).map((part, index) =>
    part.startsWith('`') && part.endsWith('`') && part.length > 1 ? (
      <code key={index} className="font-mono text-[0.875em]">
        {part.slice(1, -1)}
      </code>
    ) : (
      part
    ),
  );
}
