import { Link } from '@tanstack/react-router';
import { SiteShell, containerClass } from '@/components/site-shell';

export function NotFound() {
  return (
    <SiteShell>
      <section className={`${containerClass} flex flex-col items-start gap-5 pt-14 sm:pt-24`}>
        <h1 className="text-[40px] leading-12 font-medium tracking-[-0.025em] text-fg sm:text-5xl sm:leading-14">
          Page not found
        </h1>
        <p className="text-[17px] leading-6 text-muted">This page doesn't exist, or it moved.</p>
        <Link to="/docs/$" params={{ _splat: '' }} className="site-link text-[15px] leading-5 text-fg">
          Search the docs
        </Link>
      </section>
    </SiteShell>
  );
}
