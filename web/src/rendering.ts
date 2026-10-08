import DOMPurify from 'dompurify'
import hljs from 'highlight.js/lib/common'
import { Marked, Renderer } from 'marked'
import 'highlight.js/styles/github.css'

const markdownRenderer = new Renderer()
markdownRenderer.code = ({ text, lang }) => {
  const requested = lang?.trim().split(/\s+/, 1)[0]?.toLowerCase()
  const language = requested && hljs.getLanguage(requested) ? requested : undefined
  const html = language
    ? hljs.highlight(text, { language }).value
    : hljs.highlightAuto(text).value
  const className = language ? `hljs language-${language}` : 'hljs'
  return `<pre><code class="${className}">${html}</code></pre>\n`
}

function escapeAttribute(value: string): string {
  return value.replaceAll('&', '&amp;').replaceAll('"', '&quot;').replaceAll('<', '&lt;').replaceAll('>', '&gt;')
}

function repositoryAssetPath(href: string, markdownPath: string): string | null {
  if (/^(?:[a-z][a-z\d+.-]*:|\/\/|\/)/i.test(href)) return null
  let decoded: string
  try {
    decoded = decodeURIComponent(href.split(/[?#]/, 1)[0]).replaceAll('\\', '/')
  } catch {
    return null
  }
  const parts = markdownPath.replaceAll('\\', '/').split('/').slice(0, -1)
  for (const part of decoded.split('/')) {
    if (!part || part === '.') continue
    if (part === '..') {
      if (parts.length === 0) return null
      parts.pop()
    } else {
      parts.push(part)
    }
  }
  return parts.length ? parts.join('/') : null
}

export function renderMarkdown(source: string, repoPath: string, markdownPath: string): string {
  const renderer = new Renderer()
  renderer.code = markdownRenderer.code
  renderer.image = ({ href, title, text }) => {
    const assetPath = repositoryAssetPath(href, markdownPath)
    if (!assetPath) return `<span class="markdown-image-blocked">[图片未加载：${escapeAttribute(text)}]</span>`
    const url = `/api/asset?repo=${encodeURIComponent(repoPath)}&path=${encodeURIComponent(assetPath)}`
    const titleAttribute = title ? ` title="${escapeAttribute(title)}"` : ''
    return `<img src="${url}" alt="${escapeAttribute(text)}"${titleAttribute} loading="lazy">`
  }
  const html = new Marked({ gfm: true, breaks: false, renderer }).parse(source, { async: false }) as string
  return DOMPurify.sanitize(html, { USE_PROFILES: { html: true } })
}

const languageByExtension: Record<string, string> = {
  diff: 'diff', patch: 'diff', c: 'c', cc: 'cpp', cpp: 'cpp', cs: 'csharp', css: 'css', go: 'go', h: 'c', hpp: 'cpp',
  html: 'xml', java: 'java', js: 'javascript', json: 'json', jsx: 'javascript', kt: 'kotlin',
  md: 'markdown', mjs: 'javascript', py: 'python', rb: 'ruby', rs: 'rust', sh: 'bash',
  sql: 'sql', svg: 'xml', ts: 'typescript', tsx: 'typescript', vue: 'xml', xml: 'xml',
  yaml: 'yaml', yml: 'yaml',
}

export function renderCode(source: string, path: string): string {
  const extension = path.split('.').pop()?.toLowerCase() ?? ''
  const requested = languageByExtension[extension]
  const highlighted = requested && hljs.getLanguage(requested)
    ? hljs.highlight(source, { language: requested }).value
    : hljs.highlightAuto(source).value
  return DOMPurify.sanitize(`<code class="hljs">${highlighted}</code>`, { USE_PROFILES: { html: true } })
}

export function isMarkdownPath(path: string): boolean {
  return /\.(md|markdown|mdown|mkd)$/i.test(path)
}
