<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { isMarkdownPath, renderCode, renderMarkdown } from './rendering'

type ChangedFile = { path: string; code: string; staged: boolean; status: string }
type Repository = {
  path: string; name: string; branch: string; dirty: boolean; files: ChangedFile[]
  upstream?: string; ahead: number; behind: number; lastFetch?: string; fetchError?: string; createdAt?: number; lastModified: number
}
type RepoSort = 'name' | 'created' | 'modified'
type RootConfig = { watchRoots: string[] }
type FileContent = { path: string; preview: string; binary: boolean; truncated: boolean; deleted: boolean; diff: string }
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
const fileContent = ref<FileContent | null>(null)
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
const aiSummary = ref('')
let refreshTimer: number | undefined

const selectedRepo = computed(() => repos.value.find(repo => repo.path === selectedRepoPath.value) ?? null)
const visibleFiles = computed(() => (selectedRepo.value?.files ?? []).filter(file => file.path.toLowerCase().includes(filter.value.toLowerCase())))
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
  aiSummary.value = ''
  activeTab.value = 'diff'
  fileContent.value = null
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
async function generateAISummary() {
  if (!selectedRepo.value || !selectedFilePath.value) return
  aiBusy.value = true
  error.value = ''
  try {
    const result = await api<{ content: string }>('/api/ai/generate', {
      method: 'POST',
      body: JSON.stringify({ repo: selectedRepo.value.path, task: 'summary', path: selectedFilePath.value, apiKey: settings.value.apiKey }),
    })
    aiSummary.value = result.content
  } catch (cause) { error.value = (cause as Error).message }
  finally { aiBusy.value = false }
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
          <div class="settings-hint">API Key 会保存在本机私有配置中并在重新进入设置时回填。生成总结或 commit 信息时，只会发送所选文件的 diff，最大 512 KB。</div>
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
                    <span class="repo-indicator" :class="{ dirty: repo.dirty }"></span><span class="repo-glyph">⌘</span><span class="repo-label"><b>{{ repo.name }}</b><small>{{ repo.branch }}</small></span><span v-if="repo.behind" class="ahead-badge">↓{{ repo.behind }}</span>
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
            <button v-for="file in visibleFiles" :key="file.path" class="file-row" :class="{ chosen: selectedFilePath === file.path }" @click="selectFile(file.path)">
              <input type="checkbox" :checked="selectedFiles.includes(file.path)" @click.stop @change="toggleFile(file.path)" />
              <span class="file-status" :class="statusClass(file.code)">{{ iconFor(file.code) }}</span><span class="file-path" :title="file.path">{{ file.path.split('/').slice(-1)[0] }}<small v-if="file.path.includes('/')">{{ file.path.split('/').slice(0,-1).join('/') }}</small></span><span v-if="file.staged" class="staged-dot" title="已暂存"></span>
            </button>
            <div v-if="!visibleFiles.length" class="empty-state file-empty"><div class="empty-icon">✓</div><strong>工作区干净</strong><span>当前仓库没有待提交修改</span></div>
          </div>
          <div class="commit-box">
            <div class="commit-label"><span>COMMIT MESSAGE</span><button class="draft-button" :disabled="!selectedCount || aiBusy" @click="generateCommitDraft">{{ aiBusy ? '起草中…' : '✦ AI 起草' }}</button></div>
            <textarea v-model="commitMessage" placeholder="描述这次修改…" rows="3"></textarea>
            <button class="primary-button commit-button" :disabled="busy === 'commit' || !selectedCount || !commitMessage.trim()" @click="commitSelected"><span>{{ busy === 'commit' ? '提交中…' : `提交 ${selectedCount} 个文件` }}</span><span>↗</span></button>
          </div>
        </template>
        <div v-else class="empty-state select-repo"><div class="empty-icon">⌘</div><strong>选择一个仓库</strong><span>查看本地修改和远端状态</span></div>
      </section>

      <section class="inspector panel">
        <template v-if="selectedRepo">
          <div class="inspector-header"><div><span class="eyebrow">{{ selectedFilePath ? 'FILE INSPECTOR' : 'REPOSITORY' }}</span><h2>{{ selectedFilePath || selectedRepo.name }}</h2></div><div class="inspector-tools"><button class="outline-button" :disabled="busy === 'fetch'" @click="fetchRemote()">{{ busy === 'fetch' ? '检查中…' : '↻ 检查远端' }}</button><button v-if="selectedRepo.behind > 0" class="primary-button pull-button" :disabled="busy === 'pull' || selectedRepo.dirty" :title="selectedRepo.dirty ? '先处理本地修改' : ''" @click="pullRepo">↓ 拉取 {{ selectedRepo.behind }}</button></div></div>
          <div class="remote-strip"><span class="branch-chip"><span>⑂</span>{{ selectedRepo.branch }}</span><span v-if="selectedRepo.upstream" class="remote-text">跟踪 {{ selectedRepo.upstream }} <span v-if="selectedRepo.ahead">· ↑{{ selectedRepo.ahead }}</span><span v-if="selectedRepo.behind">· ↓{{ selectedRepo.behind }}</span><span v-if="!selectedRepo.ahead && !selectedRepo.behind">· 已同步</span></span><span v-else class="remote-text">尚未设置 upstream</span><span class="remote-time">{{ formatDate(selectedRepo.lastFetch) }}</span></div>
          <div v-if="selectedFilePath" class="tabs"><button :class="{ active: activeTab === 'diff' }" @click="activeTab = 'diff'">Diff</button><button :class="{ active: activeTab === 'preview' }" @click="activeTab = 'preview'">文件预览</button><button :class="{ active: activeTab === 'summary' }" @click="activeTab = 'summary'">✦ AI 总结</button></div>
          <template v-if="selectedFilePath && fileContent">
            <div v-if="activeTab === 'diff'" class="code-view diff-view"><pre>{{ fileContent.diff || '没有可展示的差异。' }}</pre></div>
            <div v-else-if="activeTab === 'preview'" class="code-view preview-view" :class="{ 'markdown-view': isMarkdownPreview }"><div v-if="fileContent.binary" class="inline-empty">该文件为二进制文件，暂不支持文本预览。</div><div v-else-if="fileContent.deleted" class="inline-empty">文件已删除。</div><template v-else><article v-if="isMarkdownPreview" class="markdown-content" v-html="renderedMarkdown"></article><pre v-else class="highlighted-source" :class="{ wrapped: settings.wordWrap }"><code v-html="highlightedPreview"></code></pre><div v-if="fileContent.truncated" class="truncate-note">文件较大，预览仅显示前 512 KB。</div></template></div>
            <div v-else class="ai-panel"><div class="ai-placeholder"><div class="sparkle">✦</div><h3>AI 修改总结</h3><p>生成时会把当前文件 diff 发送到已配置的模型服务。</p><span class="scope-note">{{ settings.model ? `${settings.model} · ${settings.baseUrl}` : '尚未配置模型' }}</span><button class="primary-button ai-generate-button" :disabled="aiBusy || !settings.baseUrl || !settings.model" @click="generateAISummary">{{ aiBusy ? '生成中…' : aiSummary ? '重新生成' : '生成修改总结' }}</button><button v-if="!settings.baseUrl || !settings.model" class="text-button" @click="page = 'settings'">前往模型设置</button></div><div v-if="aiSummary" class="ai-result"><span class="eyebrow">AI SUMMARY</span><pre>{{ aiSummary }}</pre></div></div>
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
