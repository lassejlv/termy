import { createRootRoute, HeadContent, Outlet, Scripts } from '@tanstack/react-router';
import * as React from 'react';
import appCss from '@/styles/app.css?url';
import { RootProvider } from 'fumadocs-ui/provider/tanstack';

export const Route = createRootRoute({
  head: () => ({
    meta: [
      {
        charSet: 'utf-8',
      },
      {
        name: 'viewport',
        content: 'width=device-width, initial-scale=1',
      },
      {
        title: 'Termy — A fast, native terminal',
      },
    ],
    links: [
      { rel: 'stylesheet', href: appCss },
      { rel: 'icon', type: 'image/svg+xml', href: '/termy-icon.svg' },
      { rel: 'icon', type: 'image/png', sizes: '512x512', href: '/termy-icon.png' },
      { rel: 'apple-touch-icon', href: '/apple-touch-icon.png' },
    ],
  }),
  component: RootComponent,
});

function RootComponent() {
  return (
    <html suppressHydrationWarning>
      <head>
        <HeadContent />
        <script
          defer
          src="https://usedatix.com/tracker.js"
          data-site="6ff1413d-87a7-498f-9fc7-22171764b7f8"
        />
      </head>
      <body className="flex flex-col min-h-screen">
        <RootProvider
          search={{ options: { type: 'static', api: '/api/search.json' } }}
          theme={{
            attribute: 'class',
            defaultTheme: 'dark',
            themes: ['light', 'dark'],
            enableSystem: false,
          }}
        >
          <Outlet />
        </RootProvider>
        <Scripts />
      </body>
    </html>
  );
}
