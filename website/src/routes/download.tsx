import { createFileRoute, Link } from '@tanstack/react-router';
import { type MouseEvent, type ReactNode, useEffect, useRef, useState } from 'react';
import {
  AppleIcon,
  MarginNote,
  SiteShell,
  containerClass,
  primaryButtonClass,
  repoUrl,
} from '@/components/site-shell';
import {
  assetArch,
  assetRow,
  fetchLatestGitHubRelease,
  formatLongDate,
  groupReleaseAssets,
  releaseVersion,
  type GitHubRelease,
  type GitHubReleaseAsset,
  type PlatformAssetGroup,
} from '@/lib/github-release';

const AUR_PACKAGE_URL = 'https://aur.archlinux.org/packages/termy-bin';
const LATEST_RELEASE_URL = `${repoUrl}/releases/latest`;

/**
 * The AUR package is published independently of the GitHub release assets, so
 * the Linux section renders even when a release attaches no Linux binaries,
 * or when the release could not be loaded at all.
 */
function withLinuxGroup(groups: PlatformAssetGroup[]): PlatformAssetGroup[] {
  if (groups.some((group) => group.id === 'linux')) return groups;

  // Keep the macOS / Linux / Windows order groupReleaseAssets produces.
  const next = [...groups];
  const windows = next.findIndex((group) => group.id === 'windows');
  next.splice(windows === -1 ? next.length : windows, 0, {
    id: 'linux',
    title: 'Linux',
    assets: [],
  });
  return next;
}

/** Signed DMGs carry a `-signed` suffix; anything else gets a warning first. */
function isUnsignedMacBuild(asset: GitHubReleaseAsset): boolean {
  const lower = asset.name.toLowerCase();
  return lower.endsWith('.dmg') && !lower.endsWith('-signed.dmg');
}

/** Apple silicon leads on macOS; everywhere else x64 comes before ARM64. */
function byArch(platform: PlatformAssetGroup['id']) {
  const first = platform === 'macos' ? 'arm64' : 'x64';
  return (a: GitHubReleaseAsset, b: GitHubReleaseAsset) =>
    Number(assetArch(b.name) === first) - Number(assetArch(a.name) === first);
}

export const Route = createFileRoute('/download')({
  component: DownloadPage,
});

type ReleaseState =
  | { status: 'loading' }
  | { status: 'loaded'; release: GitHubRelease }
  | { status: 'error' };

function DownloadPage() {
  // The repo is public, so the browser asks GitHub directly instead of going
  // through a server function that needs a token.
  const [releaseState, setReleaseState] = useState<ReleaseState>({ status: 'loading' });
  const release = releaseState.status === 'loaded' ? releaseState.release : null;
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [pendingDownload, setPendingDownload] = useState<GitHubReleaseAsset | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchLatestGitHubRelease().then(
      (release) => {
        if (!cancelled) setReleaseState({ status: 'loaded', release });
      },
      (error: unknown) => {
        console.error('Failed to load latest release', error);
        if (!cancelled) setReleaseState({ status: 'error' });
      },
    );
    return () => {
      cancelled = true;
    };
  }, []);

  const groups = withLinuxGroup(release ? groupReleaseAssets(release.assets) : []);
  const appleSilicon = groups
    .find((group) => group.id === 'macos')
    ?.assets.find((asset) => assetArch(asset.name) === 'arm64');
  const checksums = release?.assets.find((asset) => asset.name === 'checksums.txt');

  const download = (asset: GitHubReleaseAsset) => (event: MouseEvent) => {
    if (!isUnsignedMacBuild(asset)) return;
    event.preventDefault();
    setPendingDownload(asset);
    dialogRef.current?.showModal();
  };

  return (
    <SiteShell>
      <section className={`${containerClass} relative flex flex-col items-start gap-5 pt-14 sm:pt-24`}>
        <h1 className="text-[40px] leading-12 font-medium tracking-[-0.025em] text-fg sm:text-5xl sm:leading-14">
          Download Termy
        </h1>
        <ReleaseLine state={releaseState} />
        <div className="flex flex-wrap items-center gap-x-7 gap-y-3 pt-4 text-[15px] leading-5">
          <a
            href={appleSilicon?.downloadUrl ?? LATEST_RELEASE_URL}
            onClick={appleSilicon ? download(appleSilicon) : undefined}
            className={primaryButtonClass}
          >
            <AppleIcon />
            Download for Apple silicon
          </a>
          {appleSilicon && <span className="text-muted">{assetRow(appleSilicon).detail}</span>}
        </div>
        <div
          aria-hidden
          className="mt-8 aspect-[560/320] w-full max-w-[560px] min-[1180px]:absolute min-[1180px]:top-10 min-[1180px]:left-[calc(40px+620px)] min-[1180px]:mt-0 min-[1180px]:w-[560px]"
        >
          <img src="/art/laptop-light.png" alt="" width={560} height={320} className="dither dither-light size-full" />
          <img src="/art/laptop-dark.png" alt="" width={560} height={320} className="dither dither-dark size-full" />
        </div>
      </section>

      <section className={`${containerClass} flex flex-col gap-14 pt-20 sm:pt-30`}>
        {groups.map((group) => (
          <div key={group.id} id={group.id} className="relative w-full max-w-[760px] scroll-mt-8">
            <h2 className="pb-2 text-[15px] leading-5 font-normal text-muted">{group.title}</h2>
            {[...group.assets].sort(byArch(group.id)).map((asset) => {
              const row = assetRow(asset);
              return (
                <DownloadRow
                  key={asset.id}
                  label={row.label}
                  detail={row.detail}
                  action={
                    <a href={asset.downloadUrl} title={asset.name} onClick={download(asset)} className="site-link text-fg">
                      Download
                    </a>
                  }
                />
              );
            })}
            {group.id === 'linux' && (
              <DownloadRow
                label="Arch Linux"
                detail={<code className="font-mono text-[13px] sm:text-sm">yay -S termy-bin</code>}
                action={
                  <a href={AUR_PACKAGE_URL} target="_blank" rel="noreferrer" className="site-link text-fg">
                    AUR
                  </a>
                }
              />
            )}
            {group.id === 'macos' && group.assets.every((asset) => !isUnsignedMacBuild(asset)) && (
              <MarginNote top={40}>Signed and notarized.</MarginNote>
            )}
            {group.id === 'windows' && (
              <MarginNote top={40}>{'Not code-signed, so\nSmartScreen may ask first.'}</MarginNote>
            )}
          </div>
        ))}

        <div id="source" className="flex w-full max-w-[760px] scroll-mt-8 flex-col gap-3">
          <h2 className="text-[15px] leading-5 font-normal text-muted">From source</h2>
          <pre className="overflow-x-auto rounded-[10px] bg-surface px-6 py-5 font-mono text-[13px] leading-6 text-fg sm:text-sm">
            {`git clone ${repoUrl}\ncd termy\ncargo run --release -p termy`}
          </pre>
          <p className="text-[15px] leading-5 text-muted">
            Needs Rust and the platform dependencies of the desktop app.
          </p>
        </div>

        <div className="flex flex-wrap gap-x-7 gap-y-3 text-[15px] leading-5 text-fg">
          {checksums && (
            <a href={checksums.downloadUrl} className="site-link">
              checksums.txt
            </a>
          )}
          <Link to="/releases" className="site-link">
            All releases
          </Link>
          <a href={repoUrl} target="_blank" rel="noreferrer" className="site-link">
            Source on GitHub
          </a>
        </div>
      </section>

      <dialog
        ref={dialogRef}
        aria-labelledby="macos-download-warning-title"
        aria-describedby="macos-download-warning-description"
        onClose={() => setPendingDownload(null)}
        className="download-warning m-auto w-[min(32rem,calc(100%-2rem))] rounded-xl border border-rule bg-paper p-0 text-text shadow-[0_24px_60px_-12px_rgba(0,0,0,0.25)] backdrop:bg-black/40"
      >
        <div className="flex flex-col gap-4 p-6 sm:p-7">
          <h2 id="macos-download-warning-title" className="text-[21px] leading-7 font-medium tracking-[-0.01em] text-fg">
            This macOS build is unsigned.
          </h2>
          <p id="macos-download-warning-description" className="text-[15px] leading-6 text-muted">
            macOS may prevent this download from opening. Choose a signed release if one is available, or
            continue if you specifically need this unsigned build.{' '}
            <Link to="/docs/$" params={{ _splat: 'getting-started/troubleshooting' }} className="site-link text-fg">
              Troubleshooting
            </Link>
          </p>
          <p className="truncate font-mono text-[13px] leading-5 text-muted">{pendingDownload?.name}</p>
          <div className="flex items-center justify-end gap-6 pt-2 text-[15px] leading-5">
            <button
              type="button"
              onClick={() => dialogRef.current?.close()}
              className="text-muted transition-colors hover:text-fg"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={() => {
                const url = pendingDownload?.downloadUrl;
                dialogRef.current?.close();
                if (url) window.location.assign(url);
              }}
              className={primaryButtonClass}
            >
              Continue download
            </button>
          </div>
        </div>
      </dialog>
    </SiteShell>
  );
}

function ReleaseLine({ state }: { state: ReleaseState }) {
  if (state.status === 'loading') {
    return <p className="text-[17px] leading-6 text-muted">Loading the latest release…</p>;
  }

  if (state.status === 'error') {
    return (
      <p className="text-[17px] leading-6 text-muted">
        Could not reach GitHub.{' '}
        <a href={LATEST_RELEASE_URL} className="site-link text-fg">
          Download from GitHub
        </a>
      </p>
    );
  }

  const { release } = state;
  return (
    <div className="flex flex-wrap items-baseline gap-x-5 gap-y-2">
      <p className="text-[17px] leading-6 text-muted">
        {releaseVersion(release.tagName)}, released {formatLongDate(release.publishedAt)}.
      </p>
      <Link
        to="/releases/$slug"
        params={{ slug: release.tagName }}
        className="site-link text-[15px] leading-5 text-fg"
      >
        Release notes
      </Link>
    </div>
  );
}

function DownloadRow({
  label,
  detail,
  action,
}: {
  label: string;
  detail: ReactNode;
  action: ReactNode;
}) {
  return (
    <div className="grid grid-cols-[auto_minmax(16px,1fr)_auto] items-center gap-x-4 gap-y-0.5 py-3 sm:flex sm:h-13 sm:py-0">
      <span className="text-[17px] leading-6 text-fg sm:order-1">{label}</span>
      <span aria-hidden className="leader sm:order-2" />
      <span className="text-right text-[15px] leading-5 sm:order-4 sm:w-[84px]">{action}</span>
      <span className="col-span-3 text-[15px] leading-5 text-muted sm:order-3 sm:text-right">{detail}</span>
    </div>
  );
}
