import react from "@vitejs/plugin-react";
import { tanstackStart } from "@tanstack/react-start/plugin/vite";
import { defineConfig } from "vite";
import tailwindcss from "@tailwindcss/vite";
import mdx from "fumadocs-mdx/vite";
import { readdirSync } from "node:fs";

// The Markdown copy of every docs page (`/docs/<slug>.md`). Nothing links to
// these, so the crawler would not find them on its own.
function docsMarkdownPages() {
  return readdirSync("content/docs", { recursive: true, encoding: "utf8" })
    .filter((file) => file.endsWith(".mdx"))
    .map((file) => {
      // Mirrors slugsToMarkdownPath: a folder's index page is `<folder>.md`,
      // and only the docs root keeps `index.md`.
      const page = file.split("\\").join("/").replace(/\.mdx$/, "");
      const slug = page === "index" ? page : page.replace(/\/index$/, "");
      return { path: `/docs/${slug}.md` };
    });
}

// The site builds to plain static files in `dist/client`: every page is
// prerendered, and routes that cannot be (release details, unknown paths) fall
// back to the SPA shell, which renders them in the browser.
export default defineConfig({
  server: {
    port: 3000,
  },
  plugins: [
    mdx(),
    tailwindcss(),
    tanstackStart({
      prerender: {
        enabled: true,
        crawlLinks: true,
        failOnError: true,
      },
      spa: {
        enabled: true,
        // The shell renders at "/" by default, and the crawler skips a path it
        // has seen, so the homepage itself would never be prerendered.
        maskPath: "/?shell",
      },
      // Files no page links to: the static search index, the LLM text dumps
      // and the docs' Markdown copies.
      pages: [
        { path: "/api/search.json" },
        { path: "/llms.txt" },
        { path: "/llms-full.txt" },
        ...docsMarkdownPages(),
      ],
    }),
    react(),
  ],
  resolve: {
    tsconfigPaths: true,
    alias: {
      tslib: "tslib/tslib.es6.js",
    },
  },
});
