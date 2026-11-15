import { getCollection, type CollectionEntry } from 'astro:content'

export type Doc = CollectionEntry<'docs'>

// The sidebar's order, by file name in src/content/docs. Every doc must be
// listed here, and every entry must have a doc.
const groups = [
  { title: 'Get started', ids: ['index', 'install', 'welcome-tour', 'updates'] },
  {
    title: 'Using Termy Next',
    ids: ['sessions', 'termy-mux', 'panes', 'agents', 'zsh-plugins', 'command-box', 'tasks', 'ssh', 'mobile'],
  },
  { title: 'Customize', ids: ['plugins', 'themes', 'shortcuts', 'config'] },
  { title: 'Reference', ids: ['api', 'cli', 'osc-7501', 'rex'] },
]

// Reference pages are named after code, so the sidebar sets them in mono.
export const monoIds = new Set(['api', 'cli', 'termy-mux'])

export const href = (doc: Doc) => (doc.id === 'index' ? '/docs' : `/docs/${doc.id}`)

export async function loadDocs() {
  const all = await getCollection('docs')
  const byId = new Map(all.map((doc) => [doc.id, doc]))
  const sidebar = groups.map((group) => ({
    title: group.title,
    docs: group.ids.map((id) => {
      const doc = byId.get(id)
      if (!doc) throw new Error(`docs sidebar lists "${id}", but src/content/docs/${id}.md doesn't exist`)
      return doc
    }),
  }))
  const ordered = sidebar.flatMap((group) => group.docs)
  const unlisted = all.filter((doc) => !ordered.includes(doc))
  if (unlisted.length) throw new Error(`docs missing from the sidebar: ${unlisted.map((doc) => doc.id).join(', ')}`)
  const groupOf = new Map(sidebar.flatMap((group) => group.docs.map((doc) => [doc.id, group.title])))
  return { sidebar, ordered, groupOf }
}
