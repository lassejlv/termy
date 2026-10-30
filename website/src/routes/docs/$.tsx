import { createFileRoute, notFound } from '@tanstack/react-router';
import { DocsLayout } from 'fumadocs-ui/layouts/docs';
import { createServerFn } from '@tanstack/react-start';
import { staticFunctionMiddleware } from '@tanstack/start-static-server-functions';
import { slugsToMarkdownPath, source } from '@/lib/source';
import browserCollections from 'collections/browser';
import {
  DocsBody,
  DocsDescription,
  DocsPage,
  DocsTitle,
  MarkdownCopyButton,
  ViewOptionsPopover,
} from 'fumadocs-ui/layouts/docs/page';
import { baseOptions } from '@/lib/layout.shared';
import { gitConfig } from '@/lib/shared';
import { useFumadocsLoader } from 'fumadocs-core/source/client';
import { Suspense } from 'react';
import { useMDXComponents } from '@/components/mdx';
import {
  marketingFontLinks,
  marketingMono,
} from '@/components/marketing-page-shell';

export const Route = createFileRoute('/docs/$')({
  head: () => ({ links: marketingFontLinks }),
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
      markdownUrl: slugsToMarkdownPath(page.slugs).url,
      pageTree: await source.serializePageTree(source.getPageTree()),
    };
  });

const clientLoader = browserCollections.docs.createClientLoader({
  component(
    { toc, frontmatter, default: MDX },
    {
      markdownUrl,
      path,
    }: {
      markdownUrl: string;
      path: string;
    },
  ) {
    return (
      <DocsPage toc={toc} className="docs-theme-page">
        <DocsTitle
          className="text-3xl leading-tight tracking-tight text-[#e8eeff] md:text-4xl"
          style={{ fontFamily: marketingMono }}
        >
          {frontmatter.title}
        </DocsTitle>
        {frontmatter.description ? (
          <DocsDescription className="mb-0 text-balance text-[#787c99]">
            {frontmatter.description}
          </DocsDescription>
        ) : null}
        <div className="docs-theme-actions flex flex-row items-center gap-2 border-b border-fd-border pt-2 pb-6">
          <MarkdownCopyButton markdownUrl={markdownUrl} />
          <ViewOptionsPopover
            markdownUrl={markdownUrl}
            githubUrl={`https://github.com/${gitConfig.user}/${gitConfig.repo}/blob/${gitConfig.branch}/website/content/docs/${path}`}
          />
        </div>
        <DocsBody className="docs-theme-body">
          <MDX components={useMDXComponents()} />
        </DocsBody>
      </DocsPage>
    );
  },
});

function Page() {
  const { path, pageTree, markdownUrl } = useFumadocsLoader(Route.useLoaderData());

  return (
    <DocsLayout
      {...baseOptions()}
      tree={pageTree}
      containerProps={{ className: 'docs-theme' }}
    >
      <Suspense>{clientLoader.useContent(path, { markdownUrl, path })}</Suspense>
    </DocsLayout>
  );
}
