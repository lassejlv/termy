import { z } from 'zod';

import { gitConfig } from './shared';

const GITHUB_API = 'https://api.github.com';

export interface GitHubReleaseAsset {
  id: number;
  name: string;
  size: number;
  downloadUrl: string;
  contentType: string;
}

/** Termy (`v*` tags) or Termy Next, the native Swift app (`next-v*` tags). */
export type ReleaseLine = 'termy' | 'next';

export interface GitHubRelease {
  id: number;
  line: ReleaseLine;
  name: string;
  tagName: string;
  publishedAt: string;
  prerelease: boolean;
  htmlUrl: string;
  body: string | null;
  tarballUrl: string;
  zipballUrl: string;
  assets: GitHubReleaseAsset[];
}

const githubReleaseResponseSchema = z.object({
  id: z.number(),
  name: z.string().nullable(),
  tag_name: z.string(),
  published_at: z.string(),
  prerelease: z.boolean(),
  html_url: z.string(),
  body: z.string().nullable(),
  tarball_url: z.string(),
  zipball_url: z.string(),
  assets: z.array(
    z.object({
      id: z.number(),
      name: z.string(),
      size: z.number(),
      browser_download_url: z.string(),
      content_type: z.string(),
    }),
  ),
});

const githubReleaseListResponseSchema = z.array(githubReleaseResponseSchema);

type GitHubReleaseResponse = z.infer<typeof githubReleaseResponseSchema>;

function parseGitHubResponse<T>(
  schema: z.ZodType<T>,
  data: unknown,
  context: string,
): T {
  const result = schema.safeParse(data);
  if (result.success) return result.data;

  const details = result.error.issues
    .map((issue) => {
      const path = issue.path.length > 0 ? issue.path.join('.') : 'response';
      return `${path}: ${issue.message}`;
    })
    .join('; ');
  throw new Error(`GitHub API returned an invalid ${context}: ${details}`);
}

// The repo is public, so pages call GitHub from the browser without a token;
// each visitor gets their own unauthenticated rate limit.
const githubHeaders = { Accept: 'application/vnd.github+json' };

function mapGitHubRelease(data: GitHubReleaseResponse): GitHubRelease {
  return {
    id: data.id,
    line: data.tag_name.startsWith('next-') ? 'next' : 'termy',
    name: data.name || data.tag_name,
    tagName: data.tag_name,
    publishedAt: data.published_at,
    prerelease: data.prerelease,
    htmlUrl: data.html_url,
    body: data.body,
    tarballUrl: data.tarball_url,
    zipballUrl: data.zipball_url,
    assets: data.assets.map((asset) => ({
      id: asset.id,
      name: asset.name,
      size: asset.size,
      downloadUrl: asset.browser_download_url,
      contentType: asset.content_type,
    })),
  };
}

function isDesktopReleaseTag(tag: string): boolean {
  return /^(next-)?v\d/.test(tag);
}

/** `v1.0.0-rc.11` → `1.0.0-rc.11`, `next-v0.6.0` → `0.6.0`. */
export function releaseVersion(tag: string): string {
  return tag.replace(/^(next-)?v/, '');
}

export interface PlatformAssetGroup {
  id: 'macos' | 'linux' | 'windows';
  title: string;
  assets: GitHubReleaseAsset[];
}

function assetPlatform(
  name: string,
): 'macos' | 'linux' | 'windows' | 'other' {
  const lower = name.toLowerCase();
  if (lower.includes('mac') || lower.includes('darwin') || lower.endsWith('.dmg'))
    return 'macos';
  if (
    lower.includes('linux') ||
    lower.includes('appimage') ||
    lower.endsWith('.deb') ||
    lower.endsWith('.rpm')
  )
    return 'linux';
  if (
    lower.includes('windows') ||
    lower.includes('win') ||
    lower.endsWith('.msi') ||
    lower.endsWith('.exe')
  )
    return 'windows';
  return 'other';
}

/** Sidecar files attached to releases; not installers. */
function isInstallableReleaseAsset(name: string): boolean {
  const lower = name.toLowerCase();
  if (lower === 'checksums.txt') return false;
  return !(
    lower.endsWith('.sha256') ||
    lower.endsWith('.metadata') ||
    lower.endsWith('.json') ||
    lower.endsWith('.log')
  );
}

export function groupReleaseAssets(
  assets: GitHubReleaseAsset[],
): PlatformAssetGroup[] {
  const groups: PlatformAssetGroup[] = [
    { id: 'macos', title: 'macOS', assets: [] },
    { id: 'linux', title: 'Linux', assets: [] },
    { id: 'windows', title: 'Windows', assets: [] },
  ];

  for (const asset of assets) {
    if (!isInstallableReleaseAsset(asset.name)) continue;
    const platform = assetPlatform(asset.name);
    if (platform === 'other') continue;
    groups.find((group) => group.id === platform)?.assets.push(asset);
  }

  return groups.filter((group) => group.assets.length > 0);
}

export function assetArch(name: string): string | null {
  const lower = name.toLowerCase();
  if (lower.includes('aarch64') || lower.includes('arm64')) return 'arm64';
  if (lower.includes('x86_64') || lower.includes('amd64') || lower.includes('x64'))
    return 'x64';
  return null;
}

/** The label and detail of a release asset's row on the Download page. */
export function assetRow(asset: GitHubReleaseAsset): { label: string; detail: string } {
  const lower = asset.name.toLowerCase();
  const arch = assetArch(asset.name);
  const size = formatBytes(asset.size);
  const platform = assetPlatform(asset.name);

  if (platform === 'macos') {
    const label = arch === 'arm64' ? 'Apple silicon' : arch === 'x64' ? 'Intel' : 'macOS';
    return { label, detail: `DMG, ${size}` };
  }

  if (platform === 'linux') {
    const suffix = arch === 'arm64' ? ' (ARM64)' : '';
    if (lower.endsWith('.appimage'))
      return { label: 'AppImage', detail: `${arch === 'arm64' ? 'aarch64' : 'x86_64'}, ${size}` };
    if (lower.endsWith('.deb')) return { label: `Debian package${suffix}`, detail: `.deb, ${size}` };
    if (lower.endsWith('.rpm')) return { label: `RPM package${suffix}`, detail: `.rpm, ${size}` };
    if (lower.endsWith('.tar.gz') || lower.endsWith('.tgz'))
      return { label: `Archive${suffix}`, detail: `.tar.gz, ${size}` };
    return { label: `Linux${suffix}`, detail: size };
  }

  if (platform === 'windows') {
    const label = arch === 'arm64' ? 'ARM64' : arch === 'x64' ? 'x64' : 'Windows';
    const kind = lower.endsWith('.msi') ? 'MSI' : lower.includes('setup') ? 'Setup.exe' : '.exe';
    return { label, detail: `${kind}, ${size}` };
  }

  return { label: asset.name, detail: size };
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  let size = bytes / 1024;
  let unit = 'KB';
  if (size >= 1024) {
    size /= 1024;
    unit = 'MB';
  }
  if (size >= 1024) {
    size /= 1024;
    unit = 'GB';
  }
  return `${size.toFixed(size >= 10 ? 0 : 1)} ${unit}`;
}

/** `12 November 2026` */
export function formatLongDate(iso: string): string {
  return new Date(iso).toLocaleDateString('en-GB', {
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  });
}

function plainMarkdown(text: string): string {
  return text
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
    .replace(/\*\*(.+?)\*\*/g, '$1')
    .replace(/\s+/g, ' ')
    .trim();
}

/**
 * The short version of a release's notes for the Releases list: the bold
 * lead of each bullet when the notes are written that way, else the opening
 * paragraph (without its link to the full notes), else the first bullets.
 */
export function releaseHighlights(release: Pick<GitHubRelease, 'name' | 'body'>, limit = 4): string[] {
  if (!release.body) return [];
  const text = release.body.replace(/\r\n/g, '\n');

  const leads = [...text.matchAll(/^[-*] \*\*(.+?)\*\*/gm)]
    .map((match) => match[1].trim().replace(/:$/, '').replace(/([^.!?])$/, '$1.'))
    .slice(0, limit);
  if (leads.length > 0) return leads;

  const paragraph = text
    .split(/\n\s*\n|\n(?=#|[-*] )/)
    .map((part) => plainMarkdown(part.replace(/\s*Full notes:.*$/is, '')))
    .find(
      (part) =>
        part.length > 0 &&
        !part.startsWith('#') &&
        !/^[-*] /.test(part) &&
        !part.endsWith(':') &&
        !part.startsWith('Download ') &&
        !part.startsWith(release.name),
    );
  if (paragraph) return [paragraph.split(/(?<=[.!?])\s+(?=[A-Z])/)[0]];

  return [...text.matchAll(/^[-*] (.+)$/gm)]
    .map((match) => plainMarkdown(match[1].replace(/ by @\S+ in https?:\/\/\S+$/, '')))
    .slice(0, limit);
}

export async function fetchGitHubReleases(): Promise<GitHubRelease[]> {
  const res = await fetch(
    `${GITHUB_API}/repos/${gitConfig.user}/${gitConfig.repo}/releases?per_page=100`,
    { headers: githubHeaders },
  );
  if (!res.ok) {
    throw new Error(`GitHub API ${res.status}: ${await res.text()}`);
  }
  const data = parseGitHubResponse(
    githubReleaseListResponseSchema,
    await res.json(),
    'release list response',
  );
  return data
    .filter((release) => isDesktopReleaseTag(release.tag_name))
    .map(mapGitHubRelease);
}

export async function fetchGitHubReleaseByTag(
  tag: string,
): Promise<GitHubRelease | null> {
  if (!isDesktopReleaseTag(tag)) return null;

  const res = await fetch(
    `${GITHUB_API}/repos/${gitConfig.user}/${gitConfig.repo}/releases/tags/${encodeURIComponent(tag)}`,
    { headers: githubHeaders },
  );
  if (res.status === 404) return null;
  if (!res.ok) {
    throw new Error(`GitHub API ${res.status}: ${await res.text()}`);
  }
  return mapGitHubRelease(
    parseGitHubResponse(
      githubReleaseResponseSchema,
      await res.json(),
      `release response for tag ${tag}`,
    ),
  );
}

export async function fetchLatestGitHubRelease(): Promise<GitHubRelease> {
  const res = await fetch(
    `${GITHUB_API}/repos/${gitConfig.user}/${gitConfig.repo}/releases/latest`,
    { headers: githubHeaders },
  );

  if (!res.ok) {
    throw new Error(`GitHub API ${res.status}: ${await res.text()}`);
  }

  const data = parseGitHubResponse(
    githubReleaseResponseSchema,
    await res.json(),
    'latest release response',
  );

  return mapGitHubRelease(data);
}
