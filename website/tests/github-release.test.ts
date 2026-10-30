import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  fetchGitHubReleaseByTag,
  fetchGitHubReleases,
} from '../src/lib/github-release';

function release(tag: string) {
  return {
    id: 1, name: tag, tag_name: tag,
    published_at: '2026-10-02T00:00:00Z', prerelease: false,
    html_url: 'https://example.test/release', body: '',
    tarball_url: '', zipball_url: '', assets: [],
  };
}

test('release requests are fresh, unauthenticated, and desktop-only', async () => {
  const originalFetch = globalThis.fetch;
  const requests: { url: string; headers: Record<string, string> }[] = [];
  let version = 'v1.0.0';
  globalThis.fetch = Object.assign(
    async (input: string | URL | Request, init?: RequestInit) => {
      const url = String(input);
      requests.push({ url, headers: init?.headers as Record<string, string> });
      if (url.endsWith('/releases/tags/v9.9.9')) {
        return new Response('not found', { status: 404 });
      }
      if (url.includes('/releases/tags/')) return Response.json(release(version));
      return Response.json([release(version), release('sdk-v0.1.0')]);
    },
    { preconnect: originalFetch.preconnect },
  );
  try {
    assert.deepEqual(
      (await fetchGitHubReleases()).map((r) => r.tagName),
      ['v1.0.0'],
    );
    version = 'v1.0.1';
    assert.equal((await fetchGitHubReleases())[0].tagName, 'v1.0.1');

    assert.equal((await fetchGitHubReleaseByTag('v1.0.1'))?.tagName, 'v1.0.1');
    assert.equal(await fetchGitHubReleaseByTag('v9.9.9'), null);
    assert.equal(await fetchGitHubReleaseByTag('sdk-v0.1.0'), null);

    assert.equal(requests.length, 4);
    for (const { headers } of requests) {
      assert.equal(headers.Authorization, undefined);
    }
  } finally {
    globalThis.fetch = originalFetch;
  }
});
