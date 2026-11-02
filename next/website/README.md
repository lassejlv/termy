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

The halftone suns in `public/images` are generated, not photographs; the cell
map in the libtermy section (`public/images/cellmap.svg`) is drawn from the
same lines as the frame beside it.
