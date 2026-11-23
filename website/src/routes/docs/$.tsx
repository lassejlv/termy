import { createFileRoute, notFound } from '@tanstack/react-router';
import { createServerFn } from '@tanstack/react-start';
import { staticFunctionMiddleware } from '@tanstack/start-static-server-functions';
import { useFumadocsLoader } from 'fumadocs-core/source/client';
import browserCollections from 'collections/browser';
import { Suspense } from 'react';
import { DocsShell } from '@/components/docs-shell';
import { useMDXComponents } from '@/components/mdx';
import { gitConfig } from '@/lib/shared';
import { source } from '@/lib/source';

export const Route = createFileRoute('/docs/$')({
  component: Page,
  loader: async ({ params }) => {
    const slugs = params._splat?.split('/') ?? [];
    const data = await serverLoader({ data: slugs });
    await clientLoader.preload(data.path);
    return data;
  },
});

// Runs only while prerendering; the build stores each page's result as a JSON
// file that client-side navigation fetches instead of calling a server.
const serverLoader = createServerFn({
  method: 'GET',
})
  .middleware([staticFunctionMiddleware])
  .inputValidator((slugs: string[]) => slugs)
  .handler(async ({ data: slugs }) => {
    const page = source.getPage(slugs);
    if (!page) throw notFound();

    return {
      path: page.path,
      pageTree: await source.serializePageTree(source.getPageTree()),
    };
  });

const clientLoader = browserCollections.docs.createClientLoader({
  component({ toc, frontmatter, default: MDX }, { path, pageTree }: { path: string; pageTree: Parameters<typeof DocsShell>[0]['tree'] }) {
    return (
      <DocsShell
        tree={pageTree}
        toc={toc}
        title={frontmatter.title}
        description={frontmatter.description}
        editUrl={`https://github.com/${gitConfig.user}/${gitConfig.repo}/blob/${gitConfig.branch}/website/content/docs/${path}`}
      >
        <MDX components={useMDXComponents()} />
      </DocsShell>
    );
  },
});

function Page() {
  const { path, pageTree } = useFumadocsLoader(Route.useLoaderData());
  return <Suspense>{clientLoader.useContent(path, { path, pageTree })}</Suspense>;
}
