import { createFileRoute, Link } from '@tanstack/react-router';
import { Heart } from 'lucide-react';
import { SiteShell, containerClass, primaryButtonClass } from '@/components/site-shell';
import { gitConfig } from '@/lib/shared';
import { sponsors } from '@/lib/sponsors';

const sponsorUrl = `https://github.com/sponsors/${gitConfig.user}`;
const repoUrl = `https://github.com/${gitConfig.user}/${gitConfig.repo}`;

export const Route = createFileRoute('/sponsors')({
  component: SponsorsPage,
});

const FUNDS_GO: Array<[string, string]> = [
  [
    'maintenance',
    'Full-time work on Termy itself — features, bug fixes, and performance.',
  ],
  [
    'signing & infra',
    'Apple Developer membership, code signing, and build/release infrastructure.',
  ],
  [
    'community',
    'Docs, issue triage, and keeping review times short for contributors.',
  ],
];

function SponsorsPage() {
  return (
    <SiteShell>
      <section className={containerClass}>
        <div className="flex max-w-[760px] flex-col pt-14 sm:pt-24">
          <p
            className="flex items-center gap-2 text-sm text-muted"
          >
            <Heart className="size-4 text-muted" aria-hidden />
            {sponsors.length}{' '}
            {sponsors.length === 1 ? 'supporter' : 'supporters'} so far
          </p>
          <h1
            className="mt-4 text-[40px] leading-12 font-medium tracking-[-0.025em] text-fg sm:text-5xl sm:leading-14"
          >
            Sponsors
          </h1>
          <p className="mt-5 leading-relaxed text-muted">
            Termy is free and open source. Sponsorships keep development moving —
            every contribution goes straight back into the terminal.
          </p>
  
          <div className="mt-8 flex flex-wrap items-center gap-5">
            <a
              href={sponsorUrl}
              target="_blank"
              rel="noreferrer"
              className={primaryButtonClass}
            >
              Become a sponsor
            </a>
            <a
              href={repoUrl}
              target="_blank"
              rel="noreferrer"
              className="group text-[15px] text-text transition-colors hover:text-fg"
            >
              Star on GitHub{' '}
              <span
                aria-hidden
                className="inline-block transition-transform group-hover:translate-x-0.5"
              >
                →
              </span>
            </a>
          </div>
  
          <section aria-label="Current sponsors" className="mt-14">
            <h2
              className="text-[15px] leading-5 font-normal text-muted"
            >
              Current sponsors
            </h2>
            <ul className="mt-2 divide-y divide-rule">
              {sponsors.map((sponsor) => (
                <li key={sponsor.name}>
                  <a
                    href={sponsor.url}
                    target="_blank"
                    rel="noreferrer"
                    className="group flex items-center gap-4 py-4"
                  >
                    <span
                      className={`flex shrink-0 items-center ${
                        sponsor.avatar ? 'size-10 justify-center' : 'h-10 w-24'
                      }`}
                    >
                      <img
                        src={sponsor.logo.light}
                        alt={`${sponsor.name} logo`}
                        loading="lazy"
                        className={`max-h-9 max-w-full object-contain dark:hidden ${
                          sponsor.avatar ? 'rounded-full' : ''
                        }`}
                      />
                      <img
                        src={sponsor.logo.dark}
                        alt={`${sponsor.name} logo`}
                        loading="lazy"
                        className={`hidden max-h-9 max-w-full object-contain dark:block ${
                          sponsor.avatar ? 'rounded-full' : ''
                        }`}
                      />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block text-[15px] font-medium text-text transition-colors group-hover:text-fg">
                        {sponsor.name}
                      </span>
                      {sponsor.description && (
                        <span className="mt-0.5 block truncate text-xs text-muted">
                          {sponsor.description}
                        </span>
                      )}
                    </span>
                    <span
                      aria-hidden
                      className="shrink-0 text-muted transition-[transform,color] group-hover:translate-x-0.5 group-hover:text-fg"
                    >
                      ↗
                    </span>
                  </a>
                </li>
              ))}
            </ul>
          </section>
  
          <section aria-label="Where the money goes" className="mt-12">
            <h2
              className="text-[15px] leading-5 font-normal text-muted"
            >
              Where the money goes
            </h2>
            <dl className="mt-4 space-y-5">
              {FUNDS_GO.map(([term, detail]) => (
                <div key={term}>
                  <dt
                    className="text-sm text-text"
                  >
                    {term}
                  </dt>
                  <dd className="mt-1 text-[15px] leading-relaxed text-muted">
                    {detail}
                  </dd>
                </div>
              ))}
            </dl>
          </section>
  
          <section aria-label="Other ways to help" className="mt-12">
            <h2
              className="text-[15px] leading-5 font-normal text-muted"
            >
              No budget? No problem
            </h2>
            <p className="mt-4 text-[15px] leading-relaxed text-muted">
              Stars, bug reports, docs fixes, and telling a friend all help just
              as much.{' '}
              <a
                href={`${repoUrl}/issues`}
                target="_blank"
                rel="noreferrer"
                className="site-link text-fg"
              >
                Good first issues →
              </a>
            </p>
          </section>
  
          <footer
            className="mt-12 flex flex-wrap gap-x-6 gap-y-2 border-t border-rule pt-6 text-xs text-muted"
          >
            <Link to="/" className="hover:text-fg">
              ← home
            </Link>
            <a
              href={sponsorUrl}
              target="_blank"
              rel="noreferrer"
              className="hover:text-fg"
            >
              GitHub Sponsors ↗
            </a>
          </footer>
        </div>
      </section>
    </SiteShell>
  );
}
