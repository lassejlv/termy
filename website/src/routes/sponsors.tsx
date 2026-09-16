import { createFileRoute, Link } from '@tanstack/react-router';
import { Heart } from 'lucide-react';
import {
  MarketingPageShell,
  marketingFontLinks,
  marketingLinkClass,
  marketingMono,
} from '@/components/marketing-page-shell';
import { gitConfig } from '@/lib/shared';
import { sponsors } from '@/lib/sponsors';

const sponsorUrl = `https://github.com/sponsors/${gitConfig.user}`;
const repoUrl = `https://github.com/${gitConfig.user}/${gitConfig.repo}`;

export const Route = createFileRoute('/sponsors')({
  head: () => ({ links: marketingFontLinks }),
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
    <MarketingPageShell>
      <main className="mx-auto flex w-full max-w-[40rem] flex-col px-6 pt-16 pb-20 md:pt-20">
        <p
          className="flex items-center gap-2 text-sm text-[#787c99]"
          style={{ fontFamily: marketingMono }}
        >
          <Heart className="size-4 text-[#f7768e]" aria-hidden />
          {sponsors.length}{' '}
          {sponsors.length === 1 ? 'supporter' : 'supporters'} so far
        </p>
        <h1
          className="mt-4 text-4xl font-medium leading-[1.1] tracking-tight text-[#e8eeff] md:text-5xl"
          style={{ fontFamily: marketingMono }}
        >
          Sponsors
        </h1>
        <p className="mt-5 leading-relaxed text-[#787c99]">
          Termy is free and open source. Sponsorships keep development moving —
          every contribution goes straight back into the terminal.
        </p>

        <div className="mt-8 flex flex-wrap items-center gap-5">
          <a
            href={sponsorUrl}
            target="_blank"
            rel="noreferrer"
            className="rounded-full px-6 py-3 text-[15px] font-medium text-[#10192e] shadow-[0_0_35px_rgba(122,162,247,0.4),inset_0_1px_0_rgba(255,255,255,0.7)] transition-transform hover:scale-[1.02] active:scale-[0.98]"
            style={{
              background: 'linear-gradient(180deg, #eaf2ff 0%, #94b8f8 100%)',
            }}
          >
            Become a sponsor
          </a>
          <a
            href={repoUrl}
            target="_blank"
            rel="noreferrer"
            className="group text-[15px] text-[#c0caf5] transition-colors hover:text-white"
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
            className="text-[11px] font-medium tracking-[0.12em] text-[#565f89] uppercase"
            style={{ fontFamily: marketingMono }}
          >
            Current sponsors
          </h2>
          <ul className="mt-2 divide-y divide-white/[0.08]">
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
                    <span className="block text-[15px] font-medium text-[#c0caf5] transition-colors group-hover:text-white">
                      {sponsor.name}
                    </span>
                    {sponsor.description && (
                      <span className="mt-0.5 block truncate text-xs text-[#565f89]">
                        {sponsor.description}
                      </span>
                    )}
                  </span>
                  <span
                    aria-hidden
                    className="shrink-0 text-[#565f89] transition-[transform,color] group-hover:translate-x-0.5 group-hover:text-[#7aa2f7]"
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
            className="text-[11px] font-medium tracking-[0.12em] text-[#565f89] uppercase"
            style={{ fontFamily: marketingMono }}
          >
            Where the money goes
          </h2>
          <dl className="mt-4 space-y-5">
            {FUNDS_GO.map(([term, detail]) => (
              <div key={term}>
                <dt
                  className="text-sm text-[#7aa2f7]"
                  style={{ fontFamily: marketingMono }}
                >
                  {term}
                </dt>
                <dd className="mt-1 text-[15px] leading-relaxed text-[#a9b1d6]">
                  {detail}
                </dd>
              </div>
            ))}
          </dl>
        </section>

        <section aria-label="Other ways to help" className="mt-12">
          <h2
            className="text-[11px] font-medium tracking-[0.12em] text-[#565f89] uppercase"
            style={{ fontFamily: marketingMono }}
          >
            No budget? No problem
          </h2>
          <p className="mt-4 text-[15px] leading-relaxed text-[#a9b1d6]">
            Stars, bug reports, docs fixes, and telling a friend all help just
            as much.{' '}
            <a
              href={`${repoUrl}/issues`}
              target="_blank"
              rel="noreferrer"
              className={marketingLinkClass}
            >
              Good first issues →
            </a>
          </p>
        </section>

        <footer
          className="mt-12 flex flex-wrap gap-x-6 gap-y-2 border-t border-white/[0.08] pt-6 text-xs text-[#787c99]"
          style={{ fontFamily: marketingMono }}
        >
          <Link to="/" className="hover:text-white">
            ← home
          </Link>
          <a
            href={sponsorUrl}
            target="_blank"
            rel="noreferrer"
            className="hover:text-white"
          >
            GitHub Sponsors ↗
          </a>
        </footer>
      </main>
    </MarketingPageShell>
  );
}
