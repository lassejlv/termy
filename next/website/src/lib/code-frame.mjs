// Wraps each highlighted block from the docs' Markdown in a frame with a header:
// the file name from the fence (```lua title="init.lua"), else the language,
// and a Copy button the docs page wires up.

const languages = { sh: 'Shell', bash: 'Shell', zsh: 'Shell', lua: 'Lua', txt: 'Config', json: 'JSON' }

/** @returns {import('shiki').ShikiTransformer} */
export function codeFrame() {
  return {
    name: 'termy-code-frame',
    root(root) {
      const pre = root.children[0]
      if (!pre || pre.type !== 'element') return
      const lang = this.options.lang
      const title = /title="([^"]+)"/.exec(String(this.options.meta?.__raw ?? ''))?.[1]
      const label = title ?? languages[lang] ?? lang
      root.children = [
        {
          type: 'element',
          tagName: 'figure',
          properties: { className: ['code-frame'] },
          children: [
            {
              type: 'element',
              tagName: 'figcaption',
              properties: {},
              children: [
                { type: 'element', tagName: 'span', properties: { className: title ? ['code-title'] : [] }, children: [{ type: 'text', value: label }] },
                { type: 'element', tagName: 'button', properties: { type: 'button', dataCopy: '' }, children: [{ type: 'text', value: 'Copy' }] },
              ],
            },
            pre,
          ],
        },
      ]
    },
  }
}
