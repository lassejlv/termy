import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';
import type { ComponentProps } from 'react';
import { docsRoute, gitConfig } from './shared';

const marketingMono =
  "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, monospace";

function TermyNavTitle({
  href = '/',
  className,
  ...props
}: ComponentProps<'a'>) {
  return (
    <a href={href} className={className} {...props}>
      <span
        className="text-[15px] font-bold text-[#7aa2f7]"
        style={{ fontFamily: marketingMono }}
      >
        ❯_
      </span>
      <span className="text-[15px] font-medium tracking-tight">termy</span>
    </a>
  );
}

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: TermyNavTitle,
      url: '/',
    },
    links: [
      {
        text: 'Download',
        url: '/download',
      },
      {
        text: 'Docs',
        url: docsRoute,
        active: 'nested-url',
      },
      {
        text: 'Releases',
        url: '/releases',
      },
      {
        text: 'Sponsors',
        url: '/sponsors',
      },
    ],
    githubUrl: `https://github.com/${gitConfig.user}/${gitConfig.repo}`,
    themeSwitch: { mode: 'light-dark' },
  };
}
