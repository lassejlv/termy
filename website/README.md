# website

Run development server:

```bash
bun run dev
```

Build:

```bash
bun run build
```

The site is fully static. The build prerenders every page into `dist/client`
and writes `_shell.html`, a client-rendered fallback for paths that have no
prerendered file (release details and unknown paths). GitHub release data is
fetched in the browser. Serve `dist/client` from any static host that falls
back to `_shell.html` when no file matches.

## Deploying to Vercel

Deployment is configured in `vercel.json`: it builds the site, serves
`dist/client`, and rewrites paths without a file to `_shell.html`.

When importing the repo into Vercel, set the project **Root Directory** to
`website`. No other configuration is required.
