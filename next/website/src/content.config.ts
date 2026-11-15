import { defineCollection } from 'astro:content'
import { glob } from 'astro/loaders'
import { z } from 'astro/zod'

const docs = defineCollection({
  loader: glob({ pattern: '**/*.md', base: './src/content/docs' }),
  schema: z.object({
    title: z.string(),
    description: z.string(),
    // Short facts shown under the description, such as "Since 0.3.0".
    tags: z.array(z.string()).default([]),
  }),
})

export const collections = { docs }
