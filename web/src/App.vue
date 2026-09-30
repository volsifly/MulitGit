<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref } from 'vue'
import { isMarkdownPath, renderCode, renderMarkdown } from './rendering'

type ChangedFile = { path: string; originalPath?: string; code: string; staged: boolean; status: string }
type Repository = {
  path: string; name: string; branch: string; dirty: boolean; files: ChangedFile[]
  upstream?: string; ahead: number; behind: number; lastFetch?: string; fetchError?: string; createdAt?: number; lastModified: number
}
type RepoSort = 'name' | 'created' | 'modified'
type RootConfig = { watchRoots: string[] }
type FileContent = { path: string; preview: string; binary: boolean; truncated: boolean; deleted: boolean; diff: string }
type AIMessage = { role: 'user' | 'assistant'; content: string }
type CompareRow = { leftLine?: number; rightLine?: number; leftText: string; rightText: string; leftKind: 'context' | 'removed' | 'empty'; rightKind: 'context' | 'added' | 'empty' }
type AppSettings = { baseUrl: string; model: string; apiKey: string; fontSize: number; fontFamily: 'system' | 'mono'; wordWrap: boolean; commitTemplate: string; commitGuidelines: string }

const defaultSettings: AppSettings = {
  baseUrl: '', model: '', apiKey: '', fontSize: 13, fontFamily: 'system', wordWrap: true,
  commitTemplate: '<type>(<scope>): <subject>\n\n<body>\n\n<footer>',
  commitGuidelines: '- 使用中文，概括所选变更的共同目的；多个无关目的应拆分提交，不要把文件名简单拼成标题。\n- type 选择最贴切的一项：feat（新增能力）、fix（修复问题）、docs（文档）、style（纯格式）、refactor（重构）、perf（性能）、test（测试）、build（构建）、ci（持续集成）、chore（维护）、revert（回退）。\n- scope 使用所属模块或功能的简短名称；无法判断时省略。\n- subject 准确描述改动目的，避免“更新文件”“优化代码”等空泛措辞，通常不超过 50 个汉字。\n- body 说明必要背景、实现要点和影响；只写 diff 能证实的内容，不重复标题、不臆测。\n- 仅在确有不兼容变更时说明 BREAKING CHANGE；仅当 diff 提供编号时添加 issue 或工单引用。',
}

const page = ref<'workspace' | 'settings'>('workspace')
const roots = ref<string[]>([])
const repos = ref<Repository[]>([])
const selectedRepoPath = ref('')
const selectedFilePath = ref('')
const selectedFiles = ref<string[]>([])
const activeTab = ref<'diff' | 'preview' | 'summary'>('diff')
const diffMode = ref<'side-by-side' | 'unified'>('side-by-side')
const fileContent = ref<FileContent | null>(null)
const imagePreviewFailed = ref(false)
const commitMessage = ref('')
const busy = ref('')
const error = ref('')
const notice = ref('')
const filter = ref('')
const treeExpanded = ref(true)
const savedRepoSort = localStorage.getItem('mulitgit.repoSort')
const repoSort = ref<RepoSort>(savedRepoSort === 'name' || savedRepoSort === 'created' || savedRepoSort === 'modified' ? savedRepoSort : 'modified')
const settings = ref<AppSettings>({ ...defaultSettings })
const showApiKey = ref(false)
const settingsBusy = ref('')
const aiBusy = ref(false)
const aiMessages = ref<AIMessage[]>([])
const aiPrompt = ref('')
const aiScope = ref<'file' | 'all'>('file')
const aiMessagesElement = ref<HTMLElement | null>(null)
const aiContextVersion = ref(0)
let refreshTimer: number | undefined

const selectedRepo = computed(() => repos.value.find(repo => repo.path === selectedRepoPath.value) ?? null)
const visibleFiles = computed(() => (selectedRepo.value?.files ?? []).filter(file => file.path.toLowerCase().includes(filter.value.toLowerCase())))
const splitDiffRows = computed(() => parseUnifiedDiff(fileContent.value?.diff ?? ''))
const isImagePreview = computed(() => isImagePath(selectedFilePath.value))
const imagePreviewUrl = computed(() => selectedRepoPath.value && selectedFilePath.value
  ? `/api/asset?repo=${encodeURIComponent(selectedRepoPath.value)}&path=${encodeURIComponent(selectedFilePath.value)}`
  : '')
const groupedRepos = computed(() => {
  const result = new Map<string, Repository[]>()
  for (const repo of [...repos.value].sort(compareRepos)) {
    const group = parentLabel(repo.path)
    if (!result.has(group)) result.set(group, [])
    result.get(group)!.push(repo)
  }
  return [...result.entries()].sort(([groupA, itemsA], [groupB, itemsB]) => {
    if (repoSort.value === 'name') return groupA.localeCompare(groupB, 'zh-CN', { numeric: true, sensitivity: 'base' })
    const field = repoSort.value === 'created' ? 'createdAt' : 'lastModified'
    const latestA = Math.max(...itemsA.map(repo => repo[field] ?? 0))
    const latestB = Math.max(...itemsB.map(repo => repo[field] ?? 0))
    return latestB - latestA || groupA.localeCompare(groupB, 'zh-CN', { numeric: true, sensitivity: 'base' })
  })
})
const selectedCount = computed(() => selectedFiles.value.length)
const isMarkdownPreview = computed(() => isMarkdownPath(selectedFilePath.value))
const renderedMarkdown = computed(() => fileContent.value ? renderMarkdown(fileContent.value.preview, selectedRepoPath.value, selectedFilePath.value) : '')
const highlightedPreview = computed(() => fileContent.value ? renderCode(fileContent.value.preview, selectedFilePath.value) : '')
const appStyle = computed(() => ({
  '--ui-font-scale': String(settings.value.fontSize / 11),
  '--ui-font-size': `${settings.value.fontSize}px`,
  '--code-font-size': `${settings.value.fontSize}px`,
  '--code-font-family': settings.value.fontFamily === 'mono' ? "'SFMono-Regular', Consolas, monospace" : "'SFMono-Regular', Consolas, 'Liberation Mono', monospace",
  '--code-white-space': settings.value.wordWrap ? 'pre-wrap' : 'pre',
}))

function parentLabel(path: string) {
  const normalized = path.replaceAll('\\', '/')
  const parts = normalized.split('/').filter(Boolean)
  return parts.length > 1 ? parts.slice(0, -1).join('/') : path
}
function compareRepos(a: Repository, b: Repository) {
  if (repoSort.value === 'name') {
    return a.name.localeCompare(b.name, 'zh-CN', { numeric: true, sensitivity: 'base' }) || a.path.localeCompare(b.path)
  }
  const timeA = repoSort.value === 'created' ? (a.createdAt ?? 0) : a.lastModified
  const timeB = repoSort.value === 'created' ? (b.createdAt ?? 0) : b.lastModified
  return timeB - timeA || a.name.localeCompare(b.name, 'zh-CN', { numeric: true, sensitivity: 'base' })
}
function changeRepoSort() {
  localStorage.setItem('mulitgit.repoSort', repoSort.value)
}
function isImagePath(path: string) {
  return /\.(png|jpe?g|gif|webp|avif|bmp|svg)$/i.test(path)
}
function parseUnifiedDiff(diff: string): CompareRow[] {
  const rows: CompareRow[] = []
  let oldLine = 0
  let newLine = 0
  let inHunk = false
  let removed: Array<{ line: number; text: string }> = []
  let added: Array<{ line: number; text: string }> = []

  const flushChanges = () => {
    const count = Math.max(removed.length, added.length)
    for (let index = 0; index < count; index++) {
      const left = removed[index]
      const right = added[index]
      rows.push({
        leftLine: left?.line,
        rightLine: right?.line,
        leftText: left?.text ?? '',
        rightText: right?.text ?? '',
        leftKind: left ? 'removed' : 'empty',
        rightKind: right ? 'added' : 'empty',
      })
    }
    removed = []
    added = []
  }

  for (const line of diff.split('\n')) {
    if (line.startsWith('@@ ')) {
      flushChanges()
      inHunk = true
      const header = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line)
      if (header) {
        oldLine = Number(header[1])
        newLine = Number(header[2])
      }
      continue
    }
    if (!inHunk) continue
    if (line.startsWith(' ')) {
      flushChanges()
      rows.push({ leftLine: oldLine++, rightLine: newLine++, leftText: line.slice(1), rightText: line.slice(1), leftKind: 'context', rightKind: 'context' })
    } else if (line.startsWith('-')) {
      removed.push({ line: oldLine++, text: line.slice(1) })
    } else if (line.startsWith('+')) {
      added.push({ line: newLine++, text: line.slice(1) })
    } else if (!line.startsWith('\\')) {
      flushChanges()
      inHunk = false
    }
  }
  flushChanges()
  return rows
}
function shortPath(path: string) {
  const normalized = path.replaceAll('\\', '/')
  const parts = normalized.split('/').filter(Boolean)
  return parts.slice(-2).join('/') || path
}
function iconFor(code: string) {
  if (code === '??') return '＋'
  if (code.includes('D')) return '−'
  if (code.includes('A')) return '＋'
  if (code.includes('R')) return '↗'
  return 'M'
}
function statusClass(code: string) {
  if (code === '??' || code.includes('A')) return 'added'
  if (code.includes('D')) return 'deleted'
  return 'modified'
}
function formatDate(value?: string) {
  if (!value) return '尚未检查'
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString()
}
async function api<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await fetch(url, { ...init, headers: { 'Content-Type': 'application/json', ...init?.headers } })
  const payload = await response.json().catch(() => ({}))
  if (!response.ok) throw new Error(payload.error || `请求失败 (${response.status})`)
  return payload as T
}
async function loadRoots() {
  const config = await api<RootConfig>('/api/roots')
  roots.value = config.watchRoots ?? []
}
async function loadSettings() {
  settings.value = { ...defaultSettings, ...await api<AppSettings>('/api/settings') }
}
async function refreshRepos(keepSelection = true) {
  try {
    const data = await api<{ roots: string[]; repos: Repository[] }>('/api/repos')
    repos.value = data.repos ?? []
    roots.value = data.roots ?? roots.value
    if (keepSelection && selectedRepoPath.value && !repos.value.some(repo => repo.path === selectedRepoPath.value)) {
      selectedRepoPath.value = ''
      selectedFilePath.value = ''
      fileContent.value = null
    }
    if (selectedRepoPath.value) {
      const current = repos.value.find(repo => repo.path === selectedRepoPath.value)
      if (current && selectedFilePath.value && !current.files.some(file => file.path === selectedFilePath.value)) {
        selectedFilePath.value = ''
        fileContent.value = null
      }
    }
  } catch (cause) { error.value = (cause as Error).message }
}
async function addRoot() {
  const path = window.prompt('输入要监控的本地目录绝对路径')
  if (!path?.trim()) return
  error.value = ''
  busy.value = 'root'
  try {
    const config = await api<RootConfig>('/api/roots', { method: 'POST', body: JSON.stringify({ path: path.trim() }) })
    roots.value = config.watchRoots ?? []
    await refreshRepos(false)
  } catch (cause) { error.value = (cause as Error).message }
  finally { busy.value = '' }
}
async function removeRoot(index: number) {
  if (!window.confirm(`移除监控目录？\n${roots.value[index]}`)) return
  try {
    const config = await api<RootConfig>(`/api/roots?index=${index}`, { method: 'DELETE' })
    roots.value = config.watchRoots ?? []
    await refreshRepos(false)
  } catch (cause) { error.value = (cause as Error).message }
}
async function selectRepo(repo: Repository) {
  selectedRepoPath.value = repo.path
  selectedFilePath.value = ''
  aiContextVersion.value++
  aiScope.value = 'file'
  aiMessages.value = []
  aiPrompt.value = ''
  selectedFiles.value = repo.files.map(file => file.path)
  fileContent.value = null
  activeTab.value = 'diff'
  error.value = ''
  notice.value = ''
  try {
    const fresh = await api<Repository>(`/api/repo?path=${encodeURIComponent(repo.path)}`)
    updateRepo(fresh)
    selectedFiles.value = fresh.files.map(file => file.path)
    if (fresh.files.length) await selectFile(fresh.files[0].path)
    void fetchRemote(repo.path, true)
  } catch (cause) { error.value = (cause as Error).message }
}
function updateRepo(updated: Repository) {
  const index = repos.value.findIndex(repo => repo.path === updated.path)
  if (index >= 0) repos.value.splice(index, 1, { ...repos.value[index], ...updated, createdAt: updated.createdAt ?? repos.value[index].createdAt })
}
async function selectFile(path: string) {
  selectedFilePath.value = path
  aiContextVersion.value++
  aiScope.value = 'file'
  aiMessages.value = []
  aiPrompt.value = ''
  activeTab.value = 'diff'
  fileContent.value = null
  imagePreviewFailed.value = false
  if (!selectedRepoPath.value) return
  try {
    fileContent.value = await api<FileContent>(`/api/file?repo=${encodeURIComponent(selectedRepoPath.value)}&path=${encodeURIComponent(path)}`)
  } catch (cause) { error.value = (cause as Error).message }
}
function toggleFile(path: string) {
  selectedFiles.value = selectedFiles.value.includes(path)
    ? selectedFiles.value.filter(item => item !== path)
    : [...selectedFiles.value, path]
}
function toggleAll() {
  selectedFiles.value = selectedFiles.value.length === visibleFiles.value.length ? [] : visibleFiles.value.map(file => file.path)
}
async function fetchRemote(path = selectedRepoPath.value, quiet = false) {
  if (!path) return
  if (!quiet) { busy.value = 'fetch'; error.value = '' }
  try {
    const updated = await api<Repository>('/api/fetch', { method: 'POST', body: JSON.stringify({ path }) })
    updateRepo(updated)
    if (!quiet && updated.behind > 0) notice.value = `远端有 ${updated.behind} 个新提交，可选择快进拉取。`
    if (quiet && updated.behind > 0) notice.value = `该仓库远端领先 ${updated.behind} 个提交。`
  } catch (cause) {
    if (!quiet) error.value = (cause as Error).message
    else if (selectedRepoPath.value === path) notice.value = `远端检查失败：${(cause as Error).message}`
  } finally { if (!quiet) busy.value = '' }
}
async function pullRepo() {
  const repo = selectedRepo.value
  if (!repo || !window.confirm(`将对 ${repo.name} 执行 fast-forward 拉取。继续？`)) return
  busy.value = 'pull'; error.value = ''
  try {
    const updated = await api<Repository>('/api/pull', { method: 'POST', body: JSON.stringify({ path: repo.path }) })
    updateRepo(updated)
    notice.value = '拉取完成。'
    selectedFiles.value = updated.files.map(file => file.path)
    await refreshRepos()
  } catch (cause) { error.value = (cause as Error).message }
  finally { busy.value = '' }
}
async function pushRepo() {
  const repo = selectedRepo.value
  if (!repo) return
  const publishing = !repo.upstream
  const destination = publishing ? `origin/${repo.branch}` : repo.upstream
  if (!publishing && repo.behind > 0) {
    error.value = '远端已有新提交，请先拉取或处理分叉后再推送。'
    return
  }
  const confirmation = publishing
    ? `首次发布分支 ${repo.branch} 到 ${destination} 并设置 upstream？`
    : `推送 ${repo.ahead} 个本地提交到 ${destination}？`
  const details = repo.dirty ? '\n\n未提交的文件修改不会上传；只有已提交的内容会被推送。' : '\n\n只会推送已提交的内容。'
  if (!window.confirm(`${confirmation}${details}\nGit 会拒绝非快进推送，不会强制覆盖远端。`)) return
  busy.value = 'push'
  error.value = ''
  try {
    const result = await api<{ repo: Repository; published: boolean }>('/api/push', {
      method: 'POST', body: JSON.stringify({ path: repo.path }),
    })
    updateRepo(result.repo)
    notice.value = result.published ? `分支已发布到 ${result.repo.upstream || destination}，并设置了 upstream。` : `已推送到 ${result.repo.upstream || destination}。`
  } catch (cause) { error.value = (cause as Error).message }
  finally { busy.value = '' }
}
async function commitSelected() {
  const repo = selectedRepo.value
  if (!repo || !selectedFiles.value.length || !commitMessage.value.trim()) {
    error.value = '先选择文件并填写 commit 信息。'; return
  }
  const paths = selectedFiles.value.slice()
  const confirmText = `提交到 ${repo.name}：\n\n${paths.map(path => `• ${path}`).join('\n')}\n\n${commitMessage.value.trim()}`
  if (!window.confirm(confirmText)) return
  busy.value = 'commit'; error.value = ''
  try {
    const result = await api<{ hash: string; repo: Repository }>('/api/commit', {
      method: 'POST', body: JSON.stringify({ path: repo.path, files: paths, message: commitMessage.value.trim() }),
    })
    updateRepo(result.repo)
    commitMessage.value = ''
    selectedFiles.value = result.repo.files.map(file => file.path)
    notice.value = `提交完成 · ${result.hash}`
    await refreshRepos()
  } catch (cause) { error.value = (cause as Error).message }
  finally { busy.value = '' }
}
async function discardFile(file: ChangedFile) {
  const repo = selectedRepo.value
  if (!repo) return
  const consequence = file.code === '??' || file.code.includes('A')
    ? '该文件将被删除。'
    : file.status === '重命名'
      ? `文件将恢复为原路径 ${file.originalPath ?? ''}，重命名会被撤销。`
      : '该文件的暂存区和工作区修改都会恢复到最近一次提交。'
  if (!window.confirm(`撤销 ${file.path} 的修改？\n\n${consequence}\n此操作无法通过 MulitGit 撤回。`)) return
  busy.value = `discard:${file.path}`
  error.value = ''
  try {
    const updated = await api<Repository>('/api/discard', {
      method: 'POST', body: JSON.stringify({ path: repo.path, file: file.path }),
    })
    updateRepo(updated)
    selectedFiles.value = selectedFiles.value.filter(path => updated.files.some(item => item.path === path))
    if (selectedFilePath.value === file.path) {
      selectedFilePath.value = ''
      fileContent.value = null
      aiMessages.value = []
    }
    notice.value = `已撤销 ${file.path} 的修改。`
    await refreshRepos()
  } catch (cause) { error.value = (cause as Error).message }
  finally { busy.value = '' }
}
async function saveSettings() {
  settingsBusy.value = 'save'
  error.value = ''
  try {
    settings.value = await api<AppSettings>('/api/settings', { method: 'PUT', body: JSON.stringify(settings.value) })
    notice.value = '设置和 API Key 已保存到本机配置。'
  } catch (cause) { error.value = (cause as Error).message }
  finally { settingsBusy.value = '' }
}
async function testModel() {
  settingsBusy.value = 'test'
  error.value = ''
  try {
    settings.value = await api<AppSettings>('/api/settings', { method: 'PUT', body: JSON.stringify(settings.value) })
    const result = await api<{ message: string }>('/api/ai/test', { method: 'POST', body: JSON.stringify({ apiKey: settings.value.apiKey }) })
    notice.value = `模型连接成功：${result.message}`
  } catch (cause) { error.value = (cause as Error).message }
  finally { settingsBusy.value = '' }
}
async function setAIScope(scope: 'file' | 'all') {
  if (aiScope.value === scope) return
  aiScope.value = scope
  aiContextVersion.value++
  aiMessages.value = []
  aiPrompt.value = ''
  await nextTick()
  aiMessagesElement.value?.scrollTo({ top: 0 })
}
async function startAISummary(scope: 'file' | 'all') {
  await setAIScope(scope)
  aiContextVersion.value++
  aiMessages.value = []
  await sendAIMessage(scope === 'file' ? '请总结当前文件的修改内容、改动原因和可能影响。' : '请总结本仓库所有修改文件的整体内容，归纳主要改动和可能影响。')
}
function clearAIConversation() {
  aiContextVersion.value++
  aiMessages.value = []
  aiPrompt.value = ''
}
async function sendAIMessage(text = aiPrompt.value) {
  if (!selectedRepo.value || !selectedFilePath.value || aiBusy.value || (!settings.value.baseUrl || !settings.value.model)) return
  const content = text.trim()
  if (!content) return
  const previous = aiMessages.value.slice()
  const contextVersion = aiContextVersion.value
  const conversation = [...previous, { role: 'user' as const, content }]
  aiMessages.value = conversation
  aiPrompt.value = ''
  aiBusy.value = true
  error.value = ''
  await nextTick()
  if (aiMessagesElement.value) aiMessagesElement.value.scrollTop = aiMessagesElement.value.scrollHeight
  try {
    const result = await api<{ content: string }>('/api/ai/generate', {
      method: 'POST',
      body: JSON.stringify({ repo: selectedRepo.value.path, task: 'chat', scope: aiScope.value, path: selectedFilePath.value, messages: conversation.slice(-20), apiKey: settings.value.apiKey }),
    })
    if (contextVersion === aiContextVersion.value) {
      aiMessages.value.push({ role: 'assistant', content: result.content })
      await nextTick()
      if (aiMessagesElement.value) aiMessagesElement.value.scrollTop = aiMessagesElement.value.scrollHeight
    }
  } catch (cause) {
    if (contextVersion === aiContextVersion.value) {
      aiMessages.value = previous
      aiPrompt.value = content
      error.value = (cause as Error).message
    }
  } finally { aiBusy.value = false }
}
async function generateCommitDraft() {
  error.value = ''
  if (!selectedFiles.value.length) { error.value = '先选择要提交的文件。'; return }
  if (selectedRepo.value && settings.value.baseUrl && settings.value.model) {
    aiBusy.value = true
    try {
      const result = await api<{ content: string }>('/api/ai/generate', {
        method: 'POST',
        body: JSON.stringify({ repo: selectedRepo.value.path, task: 'commit', files: selectedFiles.value, apiKey: settings.value.apiKey }),
      })
      commitMessage.value = result.content
      notice.value = 'AI 已根据所选文件的 diff 起草 commit message，可继续编辑。'
    } catch (cause) { error.value = (cause as Error).message }
    finally { aiBusy.value = false }
    return
  }
  const paths = selectedFiles.value
  const kinds = new Set((selectedRepo.value?.files ?? []).filter(file => paths.includes(file.path)).map(file => file.status))
  const verb = kinds.has('新增') ? '新增' : kinds.has('删除') ? '删除' : '更新'
  const first = paths[0].split('/').slice(-1)[0]
  commitMessage.value = `${verb} ${first}${paths.length > 1 ? ` 等 ${paths.length} 个文件` : ''}`
  notice.value = '尚未配置模型，已按文件名生成本地草稿。'
}

onMounted(async () => {
  try { await Promise.all([loadRoots(), loadSettings()]); await refreshRepos(false) }
  catch (cause) { error.value = (cause as Error).message }
  refreshTimer = window.setInterval(() => void refreshRepos(), 2000)
})
onUnmounted(() => { if (refreshTimer) window.clearInterval(refreshTimer) })
</script>

<template>
  <main class="app-shell" :style="appStyle">
    <header class="topbar">
      <div class="brand"><div class="brand-mark"><img src="/mulitgit.svg" alt="" /></div><div><strong>MulitGit</strong><span>LOCAL WORKSPACE</span></div></div>
      <div class="topbar-right"><span class="local-pill"><i></i> 本地运行</span><button v-if="page === 'workspace'" class="icon-button" title="打开设置" @click="page = 'settings'">⚙</button><button v-else class="outline-button" @click="page = 'workspace'">← 返回工作台</button><button v-if="page === 'workspace'" class="icon-button" title="刷新仓库" @click="refreshRepos(false)">↻</button></div>
    </header>

    <section v-if="page === 'settings'" class="settings-page">
      <aside class="settings-nav panel">
        <span class="eyebrow">PREFERENCES</span><h2>设置</h2>
        <a class="settings-nav-item active" href="#model">模型配置</a><a class="settings-nav-item" href="#commit-template">提交信息模板</a><a class="settings-nav-item" href="#appearance">外观与编辑器</a>
      </aside>
      <div class="settings-content">
        <div class="settings-title"><span class="eyebrow">PREFERENCES</span><h1>设置</h1><p>模型连接信息和本地阅读偏好。</p></div>
        <section id="model" class="settings-card panel">
          <div class="settings-card-heading"><div><h2>模型配置</h2><p>兼容 OpenAI Chat Completions API 的服务。</p></div><span class="settings-icon">✦</span></div>
          <label class="settings-field"><span>API Base URL</span><input v-model="settings.baseUrl" placeholder="https://api.openai.com/v1" spellcheck="false" autocomplete="url" /></label>
          <label class="settings-field"><span>模型名称</span><input v-model="settings.model" placeholder="例如 gpt-4o-mini" spellcheck="false" /></label>
          <label class="settings-field"><span>API Key <small>可选，取决于服务配置</small></span><div class="secret-input"><input v-model="settings.apiKey" :type="showApiKey ? 'text' : 'password'" placeholder="输入 API Key" autocomplete="off"/><button class="secret-toggle" type="button" @click="showApiKey = !showApiKey">{{ showApiKey ? '隐藏' : '显示' }}</button></div></label>
          <div class="settings-hint">API Key 会保存在本机私有配置中并在重新进入设置时回填。AI 总结和追问只会发送当前文件或全部变更文件的 diff，最大 512 KB。</div>
          <div class="settings-actions"><button class="outline-button" :disabled="settingsBusy !== ''" @click="testModel">{{ settingsBusy === 'test' ? '连接中…' : '测试连接' }}</button><button class="primary-button" :disabled="settingsBusy !== ''" @click="saveSettings">{{ settingsBusy === 'save' ? '保存中…' : '保存模型设置' }}</button></div>
        </section>
        <section id="commit-template" class="settings-card panel">
          <div class="settings-card-heading"><div><h2>提交信息模板与规范</h2><p>分别设置提交格式和 AI 必须遵循的内容规范。</p></div><span class="settings-icon">⌘</span></div>
          <label class="settings-field"><span>Commit message 模板</span><textarea v-model="settings.commitTemplate" class="template-editor" rows="5" spellcheck="false" placeholder="例如：&#10;&lt;type&gt;(&lt;scope&gt;): &lt;subject&gt;&#10;&#10;&lt;body&gt;&#10;&#10;&lt;footer&gt;"></textarea></label>
          <div class="settings-hint">默认遵循 Conventional Commits。可用占位符：&lt;type&gt;、&lt;scope&gt;、&lt;subject&gt;、&lt;body&gt;、&lt;footer&gt;。空的正文或页脚会自动省略。</div>
          <label class="settings-field"><span>提交规范说明 <small>AI 会按这些规则判断类型、组织内容并避免空泛描述</small></span><textarea v-model="settings.commitGuidelines" class="template-editor guidelines-editor" rows="8" spellcheck="false" placeholder="说明提交信息的语言、type 选择、scope、标题、正文和页脚规则"></textarea></label>
          <div class="settings-actions"><button class="primary-button" :disabled="settingsBusy !== ''" @click="saveSettings">{{ settingsBusy === 'save' ? '保存中…' : '保存模板与规范' }}</button></div>
        </section>
        <section id="appearance" class="settings-card panel">
          <div class="settings-card-heading"><div><h2>外观与编辑器</h2><p>调整 Diff、代码和 Markdown 阅读体验。</p></div><span class="settings-icon">Aa</span></div>
          <label class="settings-field"><span>界面与代码字号 <b>{{ settings.fontSize }} px</b></span><input class="range-input" v-model.number="settings.fontSize" type="range" min="10" max="24" step="1" /></label>
          <div class="settings-hint">常规界面文字按此字号显示；辅助标签略小、标题按层级放大。代码与 Diff 使用设定的准确字号。</div>
          <label class="settings-field"><span>代码字体</span><select v-model="settings.fontFamily"><option value="system">系统等宽字体</option><option value="mono">等宽字体</option></select></label>
          <label class="settings-toggle"><span><b>代码自动换行</b><small>关闭后可横向滚动长行</small></span><input v-model="settings.wordWrap" type="checkbox" /></label>
          <div class="settings-preview"><span class="eyebrow">预览</span><pre :style="{ fontSize: `${settings.fontSize}px`, whiteSpace: settings.wordWrap ? 'pre-wrap' : 'pre' }">const change = 'readable';</pre></div>
          <div class="settings-actions"><button class="primary-button" :disabled="settingsBusy !== ''" @click="saveSettings">{{ settingsBusy === 'save' ? '保存中…' : '保存外观设置' }}</button></div>
        </section>
      </div>
    </section>

    <section v-else class="workspace">
      <aside class="sidebar panel">
        <div class="section-heading"><div><span class="eyebrow">WORKSPACES</span><h2>监控目录</h2></div><div class="sidebar-tools"><select v-model="repoSort" class="sort-select" aria-label="仓库排序方式" title="仓库排序方式" @change="changeRepoSort"><option value="name">名称</option><option value="created">创建时间</option><option value="modified">修改时间</option></select><button class="square-button header-action-button" title="添加监控目录" aria-label="添加监控目录" @click="addRoot"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg></button></div></div>
        <div class="root-list">
          <div v-for="(root, index) in roots" :key="root" class="root-block">
            <div class="root-row"><button class="disclosure" @click="treeExpanded = !treeExpanded">{{ treeExpanded ? '⌄' : '›' }}</button><span class="folder-icon">▱</span><span class="root-name" :title="root">{{ shortPath(root) }}</span><button class="root-remove" title="移除目录" @click="removeRoot(index)">×</button></div>
            <template v-if="treeExpanded">
              <template v-for="([group, items]) in groupedRepos" :key="group">
                <div v-if="items.some(repo => repo.path === root || repo.path.startsWith(root + '/') || repo.path.startsWith(root + '\\'))" class="folder-group">
                  <div class="nested-folder"><span>⌄</span><span>▱</span><span>{{ group === root ? '此目录' : shortPath(group) }}</span></div>
                  <button v-for="repo in items.filter(item => item.path === root || item.path.startsWith(root + '/') || item.path.startsWith(root + '\\'))" :key="repo.path" class="repo-row" :class="{ active: selectedRepoPath === repo.path }" @click="selectRepo(repo)">
                    <span class="repo-indicator" :class="{ dirty: repo.dirty }"></span><span class="repo-glyph">⌘</span><span class="repo-label"><b>{{ repo.name }}</b><small>{{ repo.branch }}</small></span><span v-if="repo.behind" class="ahead-badge" :title="`远端领先 ${repo.behind} 个提交，可拉取`">↓{{ repo.behind }}</span><span v-if="repo.ahead" class="ahead-badge" :title="`${repo.ahead} 个本地提交尚未推送`">↑{{ repo.ahead }}</span>
                  </button>
                </div>
              </template>
              <div v-if="!repos.some(repo => repo.path === root || repo.path.startsWith(root + '/') || repo.path.startsWith(root + '\\'))" class="empty-root">暂未发现 Git 仓库</div>
            </template>
          </div>
          <div v-if="!roots.length" class="empty-state sidebar-empty"><div class="empty-icon">⌁</div><strong>添加一个工作目录</strong><span>自动查找目录下的 Git 仓库</span><button class="text-button" @click="addRoot">＋ 添加目录</button></div>
        </div>
        <div class="sidebar-footer"><span class="footer-dot"></span>{{ repos.length }} 个仓库 · 每 15 秒刷新</div>
      </aside>

      <section class="changes panel">
        <template v-if="selectedRepo">
          <div class="section-heading changes-heading"><div><span class="eyebrow">{{ selectedRepo.branch }}</span><h2>变更文件 <span class="count-badge">{{ selectedRepo.files.length }}</span></h2></div><button class="icon-button small header-action-button" title="刷新" aria-label="刷新变更文件" @click="refreshRepos()"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 11a8 8 0 0 0-14.7-4L4 9"/><path d="M4 5v4h4"/><path d="M4 13a8 8 0 0 0 14.7 4L20 15"/><path d="M20 19v-4h-4"/></svg></button></div>
          <div class="repo-path" :title="selectedRepo.path">{{ selectedRepo.path }}</div>
          <div class="filter-wrap"><span>⌕</span><input v-model="filter" placeholder="筛选文件" /><kbd>⌘ K</kbd></div>
          <div class="file-actions"><label class="check-all"><input type="checkbox" :checked="visibleFiles.length > 0 && selectedFiles.length === visibleFiles.length" @change="toggleAll" /> 全选</label><span>{{ selectedCount }} 已选</span></div>
          <div class="file-list">
            <div v-for="file in visibleFiles" :key="file.path" class="file-row" :class="{ chosen: selectedFilePath === file.path }" role="group" :aria-label="file.path" @click="selectFile(file.path)">
              <input type="checkbox" :checked="selectedFiles.includes(file.path)" @click.stop @change="toggleFile(file.path)" />
              <span class="file-status" :class="statusClass(file.code)">{{ iconFor(file.code) }}</span><span class="file-path" :title="file.path">{{ file.path.split('/').slice(-1)[0] }}<small v-if="file.path.includes('/')">{{ file.path.split('/').slice(0,-1).join('/') }}</small></span><span v-if="file.staged" class="staged-dot" title="已暂存"></span><button class="file-discard" type="button" :title="file.status === '冲突' ? '请先处理 Git 冲突' : '撤销此文件修改'" :aria-label="file.status === '冲突' ? '请先处理 Git 冲突' : '撤销此文件修改'" :disabled="busy !== '' || file.status === '冲突'" @click.stop="discardFile(file)"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 14 4 9l5-5"/><path d="M4 9h9a7 7 0 1 1-6.2 10.2"/></svg></button>
            </div>
            <div v-if="!visibleFiles.length" class="empty-state file-empty"><div class="empty-icon">✓</div><strong>工作区干净</strong><span>当前仓库没有待提交修改</span></div>
          </div>
          <div class="commit-box">
            <div class="commit-label"><span>COMMIT MESSAGE</span><button class="draft-button" :disabled="!selectedCount || aiBusy" @click="generateCommitDraft">{{ aiBusy ? '起草中…' : '✦ AI 起草' }}</button></div>
            <textarea v-model="commitMessage" placeholder="描述这次修改…" rows="5"></textarea>
            <button class="primary-button commit-button" :disabled="busy === 'commit' || !selectedCount || !commitMessage.trim()" @click="commitSelected"><span>{{ busy === 'commit' ? '提交中…' : `提交 ${selectedCount} 个文件` }}</span><span>↗</span></button>
          </div>
        </template>
        <div v-else class="empty-state select-repo"><div class="empty-icon">⌘</div><strong>选择一个仓库</strong><span>查看本地修改和远端状态</span></div>
      </section>

      <section class="inspector panel">
        <template v-if="selectedRepo">
          <div class="inspector-header"><div><span class="eyebrow">{{ selectedFilePath ? 'FILE INSPECTOR' : 'REPOSITORY' }}</span><h2>{{ selectedFilePath || selectedRepo.name }}</h2></div><div class="inspector-tools"><button class="outline-button" :disabled="busy !== ''" @click="fetchRemote()">{{ busy === 'fetch' ? '检查中…' : '↻ 检查远端' }}</button><button v-if="selectedRepo.behind > 0" class="primary-button pull-button" :disabled="busy !== '' || selectedRepo.dirty" :title="selectedRepo.dirty ? '先处理本地修改' : ''" @click="pullRepo">↓ 拉取 {{ selectedRepo.behind }}</button><button v-if="selectedRepo.ahead > 0 || !selectedRepo.upstream" class="outline-button push-button" :disabled="busy !== '' || (Boolean(selectedRepo.upstream) && selectedRepo.behind > 0)" :title="selectedRepo.behind > 0 ? '远端已有新提交，请先同步' : ''" @click="pushRepo">{{ busy === 'push' ? '推送中…' : selectedRepo.upstream ? `↑ 推送 ${selectedRepo.ahead}` : '↑ 发布分支' }}</button></div></div>
          <div class="remote-strip"><span class="branch-chip"><span>⑂</span>{{ selectedRepo.branch }}</span><span v-if="selectedRepo.upstream" class="remote-text">跟踪 {{ selectedRepo.upstream }} <span v-if="selectedRepo.ahead">· ↑{{ selectedRepo.ahead }}</span><span v-if="selectedRepo.behind">· ↓{{ selectedRepo.behind }}</span><span v-if="!selectedRepo.ahead && !selectedRepo.behind">· 已同步</span></span><span v-else class="remote-text">尚未设置 upstream</span><span class="remote-time">{{ formatDate(selectedRepo.lastFetch) }}</span></div>
          <div v-if="selectedFilePath" class="tabs"><button :class="{ active: activeTab === 'diff' }" @click="activeTab = 'diff'">Diff</button><button :class="{ active: activeTab === 'preview' }" @click="activeTab = 'preview'">文件预览</button><button :class="{ active: activeTab === 'summary' }" @click="activeTab = 'summary'">✦ AI 总结</button></div>
          <template v-if="selectedFilePath && fileContent">
            <div v-if="activeTab === 'diff'" class="code-view diff-panel">
              <div class="diff-toolbar"><span>比较 <b>HEAD</b> 与 <b>工作区</b></span><div class="diff-mode-switch"><button :class="{ active: diffMode === 'side-by-side' }" @click="diffMode = 'side-by-side'">并排</button><button :class="{ active: diffMode === 'unified' }" @click="diffMode = 'unified'">统一</button></div></div>
              <div v-if="diffMode === 'side-by-side' && splitDiffRows.length" class="split-diff">
                <div class="split-diff-head"><span>HEAD 基线</span><span>当前工作区</span></div>
                <div v-for="(row, index) in splitDiffRows" :key="index" class="split-diff-row">
                  <div class="split-diff-cell" :class="row.leftKind"><span class="diff-line-number">{{ row.leftLine ?? '' }}</span><code>{{ row.leftText }}</code></div>
                  <div class="split-diff-cell" :class="row.rightKind"><span class="diff-line-number">{{ row.rightLine ?? '' }}</span><code>{{ row.rightText }}</code></div>
                </div>
              </div>
              <div v-else-if="diffMode === 'side-by-side'" class="inline-empty">{{ fileContent.binary || fileContent.diff.includes('Binary files') ? '二进制文件无法逐行比较，可在“文件预览”中查看图片。' : fileContent.diff || '没有可展示的差异。' }}</div>
              <div v-else class="diff-view"><pre>{{ fileContent.diff || '没有可展示的差异。' }}</pre></div>
            </div>
            <div v-else-if="activeTab === 'preview'" class="code-view preview-view" :class="{ 'markdown-view': isMarkdownPreview, 'image-preview-view': isImagePreview }">
              <div v-if="fileContent.deleted" class="inline-empty">文件已删除。</div>
              <div v-else-if="isImagePreview" class="image-preview-pane"><img v-if="!imagePreviewFailed" :src="imagePreviewUrl" :alt="selectedFilePath" @error="imagePreviewFailed = true" /><div v-else class="inline-empty">图片无法加载，可能超过 8 MiB 或格式不受支持。</div></div>
              <div v-else-if="fileContent.binary" class="inline-empty">该文件为二进制文件，暂不支持预览。</div>
              <template v-else><article v-if="isMarkdownPreview" class="markdown-content" v-html="renderedMarkdown"></article><pre v-else class="highlighted-source" :class="{ wrapped: settings.wordWrap }"><code v-html="highlightedPreview"></code></pre><div v-if="fileContent.truncated" class="truncate-note">文件较大，预览仅显示前 512 KB。</div></template>
            </div>
            <div v-else class="ai-panel ai-chat-panel">
              <div class="ai-chat-heading"><div><strong>AI 修改助手</strong><small>{{ settings.model || '尚未配置模型' }}</small></div><div class="ai-chat-tools"><div class="ai-scope-switch"><button :class="{ active: aiScope === 'file' }" @click="setAIScope('file')">当前文件</button><button :class="{ active: aiScope === 'all' }" @click="setAIScope('all')">全部修改 {{ selectedRepo.files.length }}</button></div><button v-if="aiMessages.length" class="ai-clear-button" title="清空对话" aria-label="清空对话" @click="clearAIConversation">↺</button></div></div>
              <div v-if="!settings.baseUrl || !settings.model" class="ai-config-note">先配置模型服务，才能开始总结和追问。<button class="text-button" @click="page = 'settings'">前往模型设置</button></div>
              <div ref="aiMessagesElement" class="ai-conversation">
                <div v-if="!aiMessages.length" class="ai-chat-empty"><div class="sparkle">✦</div><h3>{{ aiScope === 'file' ? '总结当前文件的修改' : '总结仓库全部修改' }}</h3><p>{{ aiScope === 'file' ? selectedFilePath : `将分析 ${selectedRepo.files.length} 个变更文件，并在对话中继续追问。` }}</p><button class="primary-button" :disabled="aiBusy || !settings.baseUrl || !settings.model || (aiScope === 'all' && !selectedRepo.files.length)" @click="startAISummary(aiScope)">{{ aiBusy ? '分析中…' : aiScope === 'file' ? '开始总结当前文件' : `总结全部 ${selectedRepo.files.length} 个文件` }}</button></div>
                <div v-for="(message, index) in aiMessages" :key="index" class="ai-turn" :class="message.role"><span class="ai-avatar">{{ message.role === 'assistant' ? '✦' : '我' }}</span><div class="ai-bubble"><article v-if="message.role === 'assistant'" class="markdown-content ai-message-markdown" v-html="renderMarkdown(message.content, selectedRepoPath, selectedFilePath || 'README.md')"></article><p v-else>{{ message.content }}</p></div></div>
                <div v-if="aiBusy" class="ai-turn assistant"><span class="ai-avatar">✦</span><div class="ai-bubble ai-thinking">正在分析…</div></div>
              </div>
              <form class="ai-composer" @submit.prevent="sendAIMessage()"><textarea v-model="aiPrompt" rows="2" :disabled="aiBusy || !settings.baseUrl || !settings.model" placeholder="继续追问，例如：这个改动会影响哪些调用方？" @keydown.ctrl.enter.prevent="sendAIMessage()" @keydown.meta.enter.prevent="sendAIMessage()"></textarea><button class="primary-button" type="submit" :disabled="aiBusy || !aiPrompt.trim() || !settings.baseUrl || !settings.model">{{ aiBusy ? '思考中…' : '发送' }}<span>↗</span></button></form>
              <div class="ai-chat-foot">{{ aiScope === 'file' ? `上下文：${selectedFilePath}` : `上下文：全部 ${selectedRepo.files.length} 个变更文件` }} · diff 最大 512 KB</div>
            </div>
          </template>
          <div v-else-if="!selectedFilePath" class="repo-overview"><div class="overview-card"><span class="overview-icon">⌘</span><div><small>当前分支</small><strong>{{ selectedRepo.branch }}</strong></div></div><div class="overview-card"><span class="overview-icon">◉</span><div><small>本地状态</small><strong>{{ selectedRepo.files.length ? `${selectedRepo.files.length} 个变更文件` : '工作区干净' }}</strong></div></div><div class="overview-card"><span class="overview-icon">⇅</span><div><small>远端状态</small><strong>{{ selectedRepo.behind ? `远端领先 ${selectedRepo.behind}` : selectedRepo.upstream ? '已同步' : '未配置 upstream' }}</strong></div></div><div class="overview-tip">选择左侧变更文件，可查看差异、预览内容和 AI 总结。</div></div>
          <div v-else class="loading-view"><span class="spinner"></span>正在读取文件…</div>
        </template>
        <div v-else class="empty-state welcome"><div class="welcome-mark">M</div><span class="eyebrow">LOCAL GIT WORKSPACE</span><h1>所有仓库，<br />一览无余。</h1><p>添加一个监控目录，MulitGit 会自动发现其中的仓库，集中查看修改并了解远端动态。</p><button class="primary-button" @click="addRoot">＋ 添加监控目录</button><div class="welcome-foot">本地运行 · Git 操作由你确认</div></div>
      </section>
    </section>
    <div v-if="error || notice" class="toast-stack"><div v-if="error" class="toast error-toast"><span>!</span>{{ error }}<button @click="error = ''">×</button></div><div v-if="notice" class="toast notice-toast"><span>✓</span>{{ notice }}<button @click="notice = ''">×</button></div></div>
  </main>
</template>
