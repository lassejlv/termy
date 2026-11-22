import { Link, useLocation } from '@tanstack/react-router';
import type { Folder, Item, Node, Root } from 'fumadocs-core/page-tree';
import { findNeighbour } from 'fumadocs-core/page-tree';
import { AnchorProvider, useActiveAnchor, type TOCItemType } from 'fumadocs-core/toc';
import { useSearchContext } from 'fumadocs-ui/contexts/search';
import { type ReactNode, useEffect, useState } from 'react';
import { SiteFooter, SiteNav, containerClass } from '@/components/site-shell';

/**
 * A folder's own page. meta.json lists `index` among the pages, so it comes
 * through as the child whose URL every sibling page extends.
 */
function folderIndex(folder: Folder): Item | undefined {
  if (folder.index) return folder.index;
  return folder.children.find(
    (child): child is Item =>
      child.type === 'page' &&
      folder.children.some((other) => other.type === 'page' && other.url.startsWith(`${child.url}/`)),
  );
}

function containsUrl(node: Node, url: string): boolean {
  if (node.type === 'page') return node.url === url;
  if (node.type === 'folder') {
    return node.children.some((child) => containsUrl(child, url)) || folderIndex(node)?.url === url;
  }
  return false;
}

/** The nearest folder that holds the page, for the label above its title. */
function findSection(tree: Root, url: string): ReactNode {
  let section: ReactNode = null;
  const walk = (nodes: Node[]) => {
    for (const node of nodes) {
      if (node.type !== 'folder' || !containsUrl(node, url)) continue;
      const index = folderIndex(node);
      if (index?.url !== url) section = index?.name ?? node.name;
      walk(node.children);
    }
  };
  walk(tree.children);
  return section;
}

/** Root nodes grouped by their separators: each separator opens a group. */
function groupNodes(nodes: Node[]) {
  const groups: { label: ReactNode; nodes: Node[] }[] = [];
  for (const node of nodes) {
    if (node.type === 'separator') groups.push({ label: node.name, nodes: [] });
    else if (groups.length === 0) groups.push({ label: null, nodes: [node] });
    else groups[groups.length - 1].nodes.push(node);
  }
  return groups;
}

function SearchButton() {
  const { enabled, setOpenSearch } = useSearchContext();
  if (!enabled) return null;
  return (
    <button
      type="button"
      onClick={() => setOpenSearch(true)}
      className="flex h-10 w-full items-center justify-between rounded-md border border-rule px-3 text-muted transition-colors hover:border-leader hover:text-text"
    >
      <span className="flex items-center gap-2 text-[15px] leading-5">
        <svg width="14" height="14" viewBox="0 0 24 24" aria-hidden className="shrink-0">
          <circle cx="11" cy="11" r="7" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
          <path d="M20 20l-3.5-3.5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
        </svg>
        Search docs
      </span>
      <kbd className="font-sans text-[13px] leading-4">⌘K</kbd>
    </button>
  );
}

function SidebarItem({ item, url, nested }: { item: Item; url: string; nested: boolean }) {
  const active = item.url === url;
  const tone = active ? 'text-text' : 'text-muted transition-colors hover:text-text';
  return (
    <Link
      to={item.url}
      aria-current={active ? 'page' : undefined}
      className={
        nested
          ? `block border-l py-1.5 pl-3.5 text-[15px] leading-5 ${active ? 'border-text font-medium' : 'border-rule'} ${tone}`
          : `block py-1.5 text-[15px] leading-5 ${active ? 'font-medium' : ''} ${tone}`
      }
    >
      {item.name}
    </Link>
  );
}

function SidebarFolder({ folder, url, nested }: { folder: Folder; url: string; nested: boolean }) {
  const index = folderIndex(folder);
  const holdsActive = containsUrl(folder, url);
  const [open, setOpen] = useState(holdsActive || folder.defaultOpen === true);

  useEffect(() => {
    if (holdsActive) setOpen(true);
  }, [holdsActive]);

  const labelClass = `block w-full py-1.5 text-left text-[15px] leading-5 ${
    holdsActive ? 'text-text' : 'text-muted transition-colors hover:text-text'
  } ${nested ? 'border-l border-rule pl-3.5' : ''}`;

  return (
    <div>
      {index ? (
        <Link
          to={index.url}
          aria-current={index.url === url ? 'page' : undefined}
          onClick={() => setOpen(true)}
          className={`${labelClass} ${index.url === url ? 'font-medium' : ''}`}
        >
          {index.name}
        </Link>
      ) : (
        <button type="button" aria-expanded={open} onClick={() => setOpen(!open)} className={labelClass}>
          {folder.name}
        </button>
      )}
      {open && (
        <div className={nested ? 'ml-3.5 border-l border-rule pl-0.5' : 'pl-0.5'}>
          <SidebarNodes
            nodes={folder.children.filter((child) => child !== index)}
            url={url}
            nested
          />
        </div>
      )}
    </div>
  );
}

function SidebarNodes({ nodes, url, nested }: { nodes: Node[]; url: string; nested: boolean }) {
  return nodes.map((node) => {
    if (node.type === 'page') return <SidebarItem key={node.url} item={node} url={url} nested={nested} />;
    if (node.type === 'folder') {
      return <SidebarFolder key={node.$id ?? String(node.name)} folder={node} url={url} nested={nested} />;
    }
    return null;
  });
}

function Sidebar({ tree, url }: { tree: Root; url: string }) {
  return (
    <nav aria-label="Documentation" className="flex flex-col gap-7">
      {groupNodes(tree.children).map((group, index) => (
        <div key={index} className="flex flex-col gap-0.5">
          {group.label && <p className="pb-1.5 text-[13px] leading-[18px] text-muted">{group.label}</p>}
          <SidebarNodes nodes={group.nodes} url={url} nested={false} />
        </div>
      ))}
    </nav>
  );
}

function TocList({ toc, title, editUrl }: { toc: TOCItemType[]; title: ReactNode; editUrl: string }) {
  const anchor = useActiveAnchor();
  const [atTop, setAtTop] = useState(true);

  useEffect(() => {
    const update = () => setAtTop(window.scrollY < 120);
    update();
    window.addEventListener('scroll', update, { passive: true });
    return () => window.removeEventListener('scroll', update);
  }, []);

  const active = atTop ? undefined : anchor;
  return (
    <nav aria-label="On this page" className="flex flex-col gap-3 text-sm leading-5">
      <p className="pt-2 text-[13px] leading-[18px] text-muted">On this page</p>
      <a href="#" className={active ? 'text-muted transition-colors hover:text-text' : 'text-text'}>
        {title}
      </a>
      {toc
        .filter((item) => item.depth <= 3)
        .map((item) => (
          <a
            key={item.url}
            href={item.url}
            className={`${item.depth === 3 ? 'pl-3' : ''} ${
              active === item.url.slice(1) ? 'text-text' : 'text-muted transition-colors hover:text-text'
            }`}
          >
            {item.title}
          </a>
        ))}
      <a href={editUrl} target="_blank" rel="noreferrer" className="site-link pt-4 text-muted">
        Edit on GitHub
      </a>
    </nav>
  );
}

export function DocsShell({
  tree,
  toc,
  title,
  description,
  editUrl,
  children,
}: {
  tree: Root;
  toc: TOCItemType[];
  title: ReactNode;
  description?: ReactNode;
  editUrl: string;
  children: ReactNode;
}) {
  const url = useLocation({ select: (location) => location.pathname.replace(/\/$/, '') || '/docs' });
  const [menuOpen, setMenuOpen] = useState(false);
  const section = findSection(tree, url);
  const { previous, next } = findNeighbour(tree, url);

  useEffect(() => setMenuOpen(false), [url]);

  return (
    <div className="flex min-h-screen flex-col bg-paper text-text">
      <SiteNav bordered />
      <AnchorProvider toc={toc}>
        <div
          className={`${containerClass} flex-1 pt-6 lg:grid lg:items-start lg:gap-[60px] lg:pt-12 min-[1024px]:max-[1179px]:grid-cols-[240px_minmax(0,1fr)] min-[1180px]:grid-cols-[240px_minmax(0,660px)_180px]`}
        >
          <aside className="lg:sticky lg:top-8">
            <div className="flex flex-col gap-3 lg:hidden">
              <SearchButton />
              <button
                type="button"
                aria-expanded={menuOpen}
                onClick={() => setMenuOpen(!menuOpen)}
                className="flex h-10 items-center justify-between rounded-md border border-rule px-3 text-[15px] leading-5 text-text"
              >
                {section ?? 'Documentation'}
                <span aria-hidden className="text-muted">
                  {menuOpen ? 'Close' : 'Menu'}
                </span>
              </button>
              {menuOpen && (
                <div className="pt-3 pb-6">
                  <Sidebar tree={tree} url={url} />
                </div>
              )}
            </div>
            <div className="hidden flex-col gap-7 lg:flex">
              <SearchButton />
              <Sidebar tree={tree} url={url} />
            </div>
          </aside>

          <article className="min-w-0 pt-10 lg:pt-1">
            <header className="flex flex-col gap-3.5">
              {section && <p className="text-[15px] leading-5 text-muted">{section}</p>}
              <h1 className="text-[32px] leading-10 font-medium tracking-[-0.02em] text-fg sm:text-[40px] sm:leading-12">
                {title}
              </h1>
              {description && <p className="text-[17px] leading-[26px] text-muted sm:text-[19px] sm:leading-7">{description}</p>}
            </header>
            <div className="docs-prose pt-9">{children}</div>
            {(previous || next) && (
              <nav aria-label="Pages" className="flex justify-between gap-6 pt-14">
                {previous ? (
                  <Link to={previous.url} className="group flex flex-col gap-1">
                    <span className="text-[13px] leading-[18px] text-muted">Previous</span>
                    <span className="text-[17px] leading-6 text-fg group-hover:underline group-hover:decoration-1 group-hover:underline-offset-[5px]">
                      {previous.name}
                    </span>
                  </Link>
                ) : (
                  <span />
                )}
                {next && (
                  <Link to={next.url} className="group flex flex-col items-end gap-1 text-right">
                    <span className="text-[13px] leading-[18px] text-muted">Next</span>
                    <span className="text-[17px] leading-6 text-fg group-hover:underline group-hover:decoration-1 group-hover:underline-offset-[5px]">
                      {next.name}
                    </span>
                  </Link>
                )}
              </nav>
            )}
          </article>

          <aside className="sticky top-8 hidden min-[1180px]:block">
            <TocList toc={toc} title={title} editUrl={editUrl} />
          </aside>
        </div>
      </AnchorProvider>
      <SiteFooter />
    </div>
  );
}
