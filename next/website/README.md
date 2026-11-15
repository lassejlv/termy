# next.termy.sh

The landing page for Termy Next: Astro with React islands, Tailwind CSS v4 and
shadcn/ui primitives. Lenis gives the page its eased scrolling, and the hero
title carries the shimmer from Arego's site.

```sh
bun install
bun run dev      # http://localhost:4321
bun run build    # astro check, then a static build in dist/
```

The download section is filled at build time from GitHub Releases
(`src/lib/release.ts`, Effect): it takes the newest non-draft `next-v*`
release and its DMG, zip and checksum assets. Set `GITHUB_TOKEN` to lift the
API rate limit. If GitHub can't be reached, the build logs a warning and uses
the last known release in that file, so rebuild after each Termy Next release
(or keep `lastKnown` current) to show the new version.

The docs at `/docs` are Markdown in `src/content/docs`, one file per page.
The sidebar's groups and order are in `src/lib/docs.ts`; the build fails if a
page is missing from it, or it lists a page that doesn't exist. Front matter
takes a `title`, a `description` (the page's lede and search text) and
optional `tags`. A code fence can name its file: ` ```lua title="init.lua" `.

The dithered light in the hero, the session and download stages and the footer
comes from the Paper design, as `src/assets/images/dither-*.png`; draw them
with `image-rendering: pixelated` (the `dither` class) so they stay crisp.
