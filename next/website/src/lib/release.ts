/**
 * The Termy Next release the download section offers, read from GitHub
 * Releases when the site is built. Termy Next ships on its own `next-v*` tags,
 * as pre-releases that never become the repository's "Latest", so the newest
 * one is found by tag rather than through `/releases/latest`.
 */
import { Config, Context, Effect, Layer, Option, Redacted, Schema } from 'effect'
import { FetchHttpClient, HttpClient, HttpClientError, HttpClientRequest, HttpClientResponse } from 'effect/http'

const REPO = 'lassejlv/termy'
const TAG = /^next-v(\d+\.\d+\.\d+(?:[-+][\w.]+)?)$/

const GitHubAsset = Schema.Struct({
  name: Schema.String,
  size: Schema.Number,
  browser_download_url: Schema.String,
})

const GitHubRelease = Schema.Struct({
  tag_name: Schema.String,
  draft: Schema.Boolean,
  prerelease: Schema.Boolean,
  published_at: Schema.NullOr(Schema.String),
  html_url: Schema.String,
  assets: Schema.Array(GitHubAsset),
})
interface GitHubRelease extends Schema.Schema.Type<typeof GitHubRelease> {}

export interface ReleaseFile {
  readonly name: string
  readonly size: number
  readonly url: string
}

export interface Release {
  readonly version: string
  readonly tag: string
  readonly prerelease: boolean
  /** ISO date, `2026-11-02`. */
  readonly publishedAt: string
  readonly notesUrl: string
  readonly dmg: ReleaseFile
  readonly zip: ReleaseFile | undefined
  readonly checksums: ReleaseFile | undefined
}

export class NoRelease extends Schema.TaggedError<NoRelease>()('ReleaseFeed.NoRelease', {
  repo: Schema.String,
}) {}

/** The last release this site was built against; served only when GitHub can't be reached. */
const lastKnown: Release = {
  version: '0.1.1',
  tag: 'next-v0.1.1',
  prerelease: true,
  publishedAt: '2026-11-02',
  notesUrl: `https://github.com/${REPO}/releases/tag/next-v0.1.1`,
  dmg: file('TermyNext-0.1.1-macos-universal.dmg', 14176085, 'next-v0.1.1'),
  zip: file('TermyNext-0.1.1-macos-universal.zip', 13141803, 'next-v0.1.1'),
  checksums: file('TermyNext-0.1.1-macos-universal.sha256', 204, 'next-v0.1.1'),
}

function file(name: string, size: number, tag: string): ReleaseFile {
  return { name, size, url: `https://github.com/${REPO}/releases/download/${tag}/${name}` }
}

function toRelease(release: GitHubRelease): Release | undefined {
  const version = TAG.exec(release.tag_name)?.[1]
  const pick = (suffix: string) => {
    const asset = release.assets.find((a) => a.name.endsWith(suffix))
    return asset && { name: asset.name, size: asset.size, url: asset.browser_download_url }
  }
  const dmg = pick('.dmg')
  if (release.draft || !version || !dmg) return undefined
  return {
    version,
    tag: release.tag_name,
    prerelease: release.prerelease,
    publishedAt: (release.published_at ?? '').slice(0, 10),
    notesUrl: release.html_url,
    dmg,
    zip: pick('.zip'),
    checksums: pick('.sha256'),
  }
}

export interface Interface {
  readonly latest: Effect.Effect<Release, NoRelease | Schema.SchemaError | HttpClientError.HttpClientError>
}

export class ReleaseFeed extends Context.Service<ReleaseFeed, Interface>()('@termy-next/ReleaseFeed') {}

export const layer = Layer.effect(
  ReleaseFeed,
  Effect.gen(function* () {
    // A token only lifts GitHub's rate limit for unauthenticated builds; it is optional.
    const token = yield* Config.option(Config.Redacted('GITHUB_TOKEN'))
    const client = (yield* HttpClient.HttpClient).pipe(
      HttpClient.mapRequest((request) =>
        Option.match(token, {
          onNone: () => request,
          onSome: (secret) => HttpClientRequest.bearerToken(request, Redacted.value(secret)),
        }).pipe(HttpClientRequest.acceptJson, HttpClientRequest.setHeader('x-github-api-version', '2022-11-28')),
      ),
      HttpClient.filterStatusOk,
      HttpClient.retryTransient({ times: 3 }),
    )

    const latest = Effect.fn('ReleaseFeed.latest')(function* () {
      const response = yield* client.get(`https://api.github.com/repos/${REPO}/releases?per_page=50`)
      const releases = yield* HttpClientResponse.schemaBodyJson(Schema.Array(GitHubRelease))(response)
      // GitHub lists releases newest first.
      for (const release of releases) {
        const found = toRelease(release)
        if (found) return found
      }
      return yield* new NoRelease({ repo: REPO })
    })

    return ReleaseFeed.of({ latest: latest() })
  }),
)

/** The release to offer, falling back to the last known one when GitHub can't be read. */
export const loadRelease = (): Promise<Release> =>
  Effect.gen(function* () {
    const feed = yield* ReleaseFeed
    return yield* feed.latest
  }).pipe(
    Effect.catch((error) =>
      Effect.logWarning(`Using the last known Termy Next release (${lastKnown.tag}): ${String(error)}`).pipe(
        Effect.as(lastKnown),
      ),
    ),
    Effect.provide(layer.pipe(Layer.provide(FetchHttpClient.layer))),
    Effect.runPromise,
  )

/** Sizes as Finder shows them, in decimal units. */
export function formatSize(bytes: number): string {
  if (bytes < 1000) return `${bytes} B`
  if (bytes < 1000 * 1000) return `${(bytes / 1000).toFixed(1)} KB`
  return `${(bytes / 1000 / 1000).toFixed(1)} MB`
}

export function formatDate(iso: string): string {
  const date = new Date(`${iso}T00:00:00Z`)
  return date.toLocaleDateString('en-GB', { day: 'numeric', month: 'long', year: 'numeric', timeZone: 'UTC' })
}
