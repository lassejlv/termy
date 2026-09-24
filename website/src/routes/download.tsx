import { createFileRoute, Link } from '@tanstack/react-router';
import { createServerFn } from '@tanstack/react-start';
import { TriangleAlert } from 'lucide-react';
import { useRef, useState } from 'react';
import {
  MarketingPageShell,
  marketingFontLinks,
  marketingLinkClass,
  marketingMono,
} from '@/components/marketing-page-shell';
import {
  assetArch,
  assetLabel,
  fetchLatestGitHubRelease,
  formatBytes,
  formatReleaseDate,
  groupReleaseAssets,
  type GitHubRelease,
  type GitHubReleaseAsset,
  type PlatformAssetGroup,
} from '@/lib/github-release';

const AUR_PACKAGE_URL = 'https://aur.archlinux.org/packages/termy-bin';

/**
 * The AUR package is published independently of the GitHub release assets, so
 * the Linux section renders even when a release attaches no Linux binaries —
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

const loadDownloadReleases = createServerFn({ method: 'GET' }).handler(
  async () => {
    try {
      return {
        release: await fetchLatestGitHubRelease(),
        error: null as string | null,
      };
    } catch (error) {
      return {
        release: null as GitHubRelease | null,
        error:
          error instanceof Error ? error.message : 'Failed to load latest release',
      };
    }
  },
);

export const Route = createFileRoute('/download')({
  head: () => ({ links: marketingFontLinks }),
  component: DownloadPage,
  loader: () => loadDownloadReleases(),
});

function DownloadPage() {
  const { release, error } = Route.useLoaderData();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [pendingDownload, setPendingDownload] = useState<{
    name: string;
    url: string;
  } | null>(null);

  const groups = withLinuxGroup(
    release ? groupReleaseAssets(release.assets) : [],
  );
  const githubUrl =
    release?.htmlUrl ?? 'https://github.com/lassejlv/termy/releases';

  const warnBeforeMacDownload = (name: string, url: string) => {
    setPendingDownload({ name, url });
    dialogRef.current?.showModal();
  };

  const continueMacDownload = () => {
    const url = pendingDownload?.url;
    dialogRef.current?.close();
    if (url) window.location.assign(url);
  };

  return (
    <MarketingPageShell>
      <main className="mx-auto flex w-full max-w-[40rem] flex-col px-6 pt-16 pb-20 md:pt-20">
        <h1
          className="text-4xl font-medium leading-none tracking-tight text-[#e8eeff] md:text-5xl"
          style={{ fontFamily: marketingMono }}
        >
          Download
        </h1>

        {release && (
          <p
            className="mt-5 text-sm text-[#787c99]"
            style={{ fontFamily: marketingMono }}
          >
            <span className="text-[#c0caf5]">{release.tagName}</span>
            {' · '}
            {formatReleaseDate(release.publishedAt)}
            {' · '}
            <Link to="/releases" className={marketingLinkClass}>
              release notes
            </Link>
          </p>
        )}

        <div className="mt-12">
          <AssetPanel
            error={error}
            release={release}
            groups={groups}
            githubUrl={githubUrl}
            onMacDownload={warnBeforeMacDownload}
          />
        </div>

        <footer
          className="mt-8 flex flex-wrap gap-x-6 gap-y-2 text-xs text-[#787c99]"
          style={{ fontFamily: marketingMono }}
        >
          <Link to="/releases" className="hover:text-white">
            all releases →
          </Link>
          <a
            href={githubUrl}
            target="_blank"
            rel="noreferrer"
            className="hover:text-white"
          >
            GitHub ↗
          </a>
          {release && (
            <a href={release.tarballUrl} className="hover:text-white">
              source tarball ↓
            </a>
          )}
        </footer>

        <dialog
          ref={dialogRef}
          aria-labelledby="macos-download-warning-title"
          aria-describedby="macos-download-warning-description"
          onClose={() => setPendingDownload(null)}
          className="download-warning m-auto w-[min(34rem,calc(100%-2rem))] rounded-2xl border border-white/[0.1] bg-[#16161e] p-0 text-[#c0caf5] shadow-[0_30px_100px_rgba(0,0,0,0.55)] backdrop:bg-[#080a10]/70 backdrop:backdrop-blur-sm"
        >
          <div className="p-6 sm:p-7">
            <div className="flex items-start gap-4">
              <span className="flex size-10 shrink-0 items-center justify-center rounded-full border border-[#7aa2f7]/25 bg-[#7aa2f7]/10 text-[#7aa2f7]">
                <TriangleAlert className="size-5" />
              </span>
              <div className="min-w-0">
                <p
                  className="text-[10px] text-[#565f89]"
                  style={{ fontFamily: marketingMono }}
                >
                  macOS installation
                </p>
                <h2
                  id="macos-download-warning-title"
                  className="mt-1 text-2xl font-medium leading-tight text-[#e8eeff]"
                  style={{ fontFamily: marketingMono }}
                >
                  This macOS build is unsigned.
                </h2>
              </div>
            </div>

            <p
              id="macos-download-warning-description"
              className="mt-5 text-sm leading-relaxed text-[#787c99]"
            >
              macOS may prevent this download from opening. Choose a signed
              release if one is available, or continue if you specifically need
              this unsigned build.
            </p>

            <a
              href="https://termy.sh/docs/getting-started/troubleshooting"
              target="_blank"
              rel="noreferrer"
              className="mt-4 inline-block text-xs text-[#7aa2f7] underline decoration-[#7aa2f7]/35 underline-offset-4 hover:decoration-[#7aa2f7]"
              style={{ fontFamily: marketingMono }}
            >
              Read the troubleshooting guide ↗
            </a>

            <div className="mt-7 flex flex-col-reverse gap-3 border-t border-white/[0.08] pt-5 sm:flex-row sm:items-center sm:justify-between">
              <p
                className="min-w-0 truncate text-[10px] text-[#565f89]"
                style={{ fontFamily: marketingMono }}
              >
                {pendingDownload?.name}
              </p>
              <div className="flex shrink-0 gap-3">
                <button
                  type="button"
                  onClick={() => dialogRef.current?.close()}
                  className="rounded-full border border-white/[0.1] px-4 py-2 text-xs text-[#787c99] transition-colors hover:text-white active:scale-[0.97]"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={continueMacDownload}
                  className="rounded-full bg-[#9fc0ff] px-4 py-2 text-xs font-medium text-[#10192e] transition-transform hover:scale-[1.02] active:scale-[0.97]"
                >
                  Continue download
                </button>
              </div>
            </div>
          </div>
        </dialog>
      </main>
    </MarketingPageShell>
  );
}

function AssetPanel({
  error,
  release,
  groups,
  githubUrl,
  onMacDownload,
}: {
  error: string | null;
  release: GitHubRelease | null;
  groups: PlatformAssetGroup[];
  githubUrl: string;
  onMacDownload: (name: string, url: string) => void;
}) {
  // The sections still render underneath a notice, because the Arch Linux
  // install command does not depend on the release having loaded.
  const notice = error ? (
    <p className="pb-8 font-mono text-sm text-fd-muted-foreground">
      <span className="text-fd-error">error:</span> could not reach GitHub.{' '}
      <a
        href="https://github.com/lassejlv/termy/releases/latest"
        target="_blank"
        rel="noreferrer"
        className={marketingLinkClass}
      >
        Download from GitHub →
      </a>
    </p>
  ) : !release ? (
    <p className="pb-8 font-mono text-sm text-fd-muted-foreground">
      No release published yet.{' '}
      <a
        href={githubUrl}
        target="_blank"
        rel="noreferrer"
        className={marketingLinkClass}
      >
        View on GitHub →
      </a>
    </p>
  ) : groups.every((group) => group.assets.length === 0) ? (
    <p className="pb-8 font-mono text-sm text-fd-muted-foreground">
      No binaries for this release yet.{' '}
      <a
        href={githubUrl}
        target="_blank"
        rel="noreferrer"
        className={marketingLinkClass}
      >
        View on GitHub →
      </a>
    </p>
  ) : null;

  return (
    <>
      {notice}
      <div className="divide-y divide-white/[0.08]">
        {groups.map((group) => (
          <section key={group.id} className="py-7 first:pt-2 last:pb-2">
            <h2
              className="text-[11px] font-medium tracking-[0.12em] text-[#565f89] uppercase"
              style={{ fontFamily: marketingMono }}
            >
              {group.title}
            </h2>
            {group.assets.length > 0 && (
              <ul className="mt-3">
                {group.assets.map((asset) => {
                  const arch = assetArch(asset.name);
                  return (
                    <li key={asset.id}>
                      <a
                        href={asset.downloadUrl}
                        title={asset.name}
                        onClick={(event) => {
                          if (
                            group.id === 'macos' &&
                            (arch === 'arm64' || arch === 'x64') &&
                            !asset.name.toLowerCase().endsWith('-signed.dmg')
                          ) {
                            event.preventDefault();
                            onMacDownload(asset.name, asset.downloadUrl);
                          }
                        }}
                        className="group flex items-center gap-4 py-3 transition-colors hover:text-white"
                      >
                        <span className="min-w-0 flex-1 text-[15px] font-medium text-[#c0caf5] transition-colors group-hover:text-white">
                          {assetLabel(asset.name)}
                        </span>
                        {arch && (
                          <span
                            className="hidden w-12 shrink-0 text-xs text-[#565f89] sm:block"
                            style={{ fontFamily: marketingMono }}
                          >
                            {arch}
                          </span>
                        )}
                        <span
                          className="w-14 shrink-0 text-right text-xs text-[#787c99] tabular-nums"
                          style={{ fontFamily: marketingMono }}
                        >
                          {formatBytes(asset.size)}
                        </span>
                      </a>
                    </li>
                  );
                })}
              </ul>
            )}
            {group.id === 'linux' && (
              <LinuxInstallHints assets={group.assets} />
            )}
          </section>
        ))}
      </div>
    </>
  );
}

function LinuxInstallHints({ assets }: { assets: GitHubReleaseAsset[] }) {
  const deb = assets.find((asset) => asset.name.toLowerCase().endsWith('.deb'));
  const rpm = assets.find((asset) => asset.name.toLowerCase().endsWith('.rpm'));

  return (
    <div className="mt-4 flex flex-col gap-3">
      {deb && (
        <InstallCommand
          label="Debian / Ubuntu"
          command={`sudo apt install ./${deb.name}`}
        />
      )}
      {rpm && (
        <InstallCommand
          label="Fedora / RHEL"
          command={`sudo dnf install ./${rpm.name}`}
        />
      )}
      {/* Packaged in the AUR rather than attached to the release. */}
      <InstallCommand
        label="Arch Linux (AUR)"
        command="yay -S termy-bin"
        href={AUR_PACKAGE_URL}
      />
    </div>
  );
}

function InstallCommand({
  label,
  command,
  href,
}: {
  label: string;
  command: string;
  href?: string;
}) {
  return (
    <div>
      {href ? (
        <a
          href={href}
          target="_blank"
          rel="noreferrer"
          className="inline-block text-[10px] text-[#565f89] transition-colors hover:text-[#7aa2f7]"
          style={{ fontFamily: marketingMono }}
        >
          {label} ↗
        </a>
      ) : (
        <p
          className="text-[10px] text-[#565f89]"
          style={{ fontFamily: marketingMono }}
        >
          {label}
        </p>
      )}
      <pre
        className="mt-1 overflow-x-auto rounded-xl border border-white/[0.08] bg-[#0d0f17] px-4 py-3 text-xs leading-relaxed text-[#9ece6a]"
        style={{ fontFamily: marketingMono }}
      >
        <code>{command}</code>
      </pre>
    </div>
  );
}
