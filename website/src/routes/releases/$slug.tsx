import { createFileRoute, Link } from '@tanstack/react-router';
import { useEffect, useState } from 'react';
import {
  MarketingPageShell,
  marketingFontLinks,
  marketingLinkClass,
  marketingMono,
} from '@/components/marketing-page-shell';
import {
  fetchGitHubReleaseByTag,
  formatReleaseDate,
  type GitHubRelease,
} from '@/lib/github-release';
import { Markdown } from '@/components/markdown';

export const Route = createFileRoute('/releases/$slug')({
  head: () => ({ links: marketingFontLinks }),
  component: ReleaseDetail,
});

type ReleaseState =
  | { status: 'loading' }
  | { status: 'loaded'; release: GitHubRelease }
  | { status: 'not-found' }
  | { status: 'error' };

function ReleaseDetail() {
  const { slug } = Route.useParams();
  const [state, setState] = useState<ReleaseState>({ status: 'loading' });

  useEffect(() => {
    let cancelled = false;
    setState({ status: 'loading' });
    fetchGitHubReleaseByTag(slug).then(
      (release) => {
        if (cancelled) return;
        setState(
          release ? { status: 'loaded', release } : { status: 'not-found' },
        );
      },
      (error: unknown) => {
        console.error(`Failed to load release ${slug}`, error);
        if (!cancelled) setState({ status: 'error' });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [slug]);

  const release = state.status === 'loaded' ? state.release : null;

  return (
    <MarketingPageShell>
      <main className="mx-auto flex w-full max-w-[40rem] flex-col px-6 pt-16 pb-20 md:pt-20">
        <Link
          to="/releases"
          className="text-xs text-[#787c99] hover:text-white"
          style={{ fontFamily: marketingMono }}
        >
          ← all releases
        </Link>

        {state.status === 'loading' && (
          <p className="mt-10 font-mono text-sm text-fd-muted-foreground">
            Loading {slug}…
          </p>
        )}

        {state.status === 'not-found' && (
          <p className="mt-10 font-mono text-sm text-fd-muted-foreground">
            No release named {slug}.
          </p>
        )}

        {state.status === 'error' && (
          <p className="mt-10 font-mono text-sm text-fd-muted-foreground">
            <span className="text-fd-error">error:</span> could not load this
            release.{' '}
            <a
              href={`https://github.com/lassejlv/termy/releases/tag/${encodeURIComponent(slug)}`}
              target="_blank"
              rel="noreferrer"
              className={marketingLinkClass}
            >
              View on GitHub →
            </a>
          </p>
        )}

        {release && (
          <>
          <article className="mt-10">
            <time
              dateTime={release.publishedAt}
              className="text-xs text-[#787c99]"
              style={{ fontFamily: marketingMono }}
            >
              {formatReleaseDate(release.publishedAt)}
            </time>
            <h1
              className="mt-3 text-4xl font-medium leading-none tracking-tight text-[#e8eeff] md:text-5xl"
              style={{ fontFamily: marketingMono }}
            >
              {release.tagName}
            </h1>

            <div className="release-notes prose prose-invert mt-10 max-w-none">
              <Markdown
                text={release.body || '_No release notes were provided._'}
              />
            </div>
          </article>

          <div
            className="mt-8 flex flex-wrap gap-x-6 gap-y-2 text-xs text-[#787c99]"
            style={{ fontFamily: marketingMono }}
          >
            <a
              href={release.htmlUrl}
              target="_blank"
              rel="noreferrer"
              className="hover:text-white"
            >
              View on GitHub ↗
            </a>
            <a href={release.tarballUrl} className="hover:text-white">
              Source tarball ↓
            </a>
          </div>
          </>
        )}
      </main>
    </MarketingPageShell>
  );
}
