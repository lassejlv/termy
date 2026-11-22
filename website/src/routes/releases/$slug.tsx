import { createFileRoute } from '@tanstack/react-router';
import { useEffect, useState } from 'react';
import { Markdown } from '@/components/markdown';
import { SiteShell, containerClass, repoUrl } from '@/components/site-shell';
import {
  fetchGitHubReleaseByTag,
  formatLongDate,
  releaseVersion,
  type GitHubRelease,
} from '@/lib/github-release';

export const Route = createFileRoute('/releases/$slug')({
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
        setState(release ? { status: 'loaded', release } : { status: 'not-found' });
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
    <SiteShell>
      <article className={`${containerClass} flex flex-col items-start pt-14 sm:pt-24`}>
        <a
          href={release?.line === 'next' ? '/releases?line=next' : '/releases'}
          className="site-link text-[15px] leading-5 text-muted"
        >
          All releases
        </a>

        {state.status === 'loading' && <p className="mt-10 text-[17px] leading-6 text-muted">Loading {slug}…</p>}
        {state.status === 'not-found' && (
          <p className="mt-10 text-[17px] leading-6 text-muted">No release named {slug}.</p>
        )}
        {state.status === 'error' && (
          <p className="mt-10 text-[17px] leading-6 text-muted">
            Could not load this release.{' '}
            <a href={`${repoUrl}/releases/tag/${encodeURIComponent(slug)}`} className="site-link text-fg">
              View it on GitHub
            </a>
          </p>
        )}

        {release && (
          <>
            <h1 className="mt-8 text-[40px] leading-12 font-medium tracking-[-0.025em] text-fg sm:text-5xl sm:leading-14">
              {release.line === 'next' ? 'Termy Next ' : ''}
              {releaseVersion(release.tagName)}
            </h1>
            <p className="mt-5 text-[17px] leading-6 text-muted">
              {release.prerelease ? 'Pre-release, released' : 'Released'} {formatLongDate(release.publishedAt)}.
            </p>
            <div className="docs-prose mt-10 w-full max-w-[760px]">
              <Markdown text={release.body || '_No release notes were provided._'} />
            </div>
            <div className="mt-12 flex flex-wrap gap-x-7 gap-y-3 text-[15px] leading-5 text-fg">
              <a href={release.htmlUrl} target="_blank" rel="noreferrer" className="site-link">
                View on GitHub
              </a>
              <a href={release.tarballUrl} className="site-link">
                Source tarball
              </a>
            </div>
          </>
        )}
      </article>
    </SiteShell>
  );
}
