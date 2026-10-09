<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import '@xterm/xterm/css/xterm.css'

const props = defineProps<{ repo: string; active: boolean; fontSize: number; fontFamily: string }>()
const emit = defineEmits<{ repo: [value: any] }>()
const container = ref<HTMLElement | null>(null)
const status = ref('连接中')
const connected = ref(false)
const fullscreen = ref(false)
let term: Terminal
let fit: FitAddon
let socket: WebSocket | undefined
let observer: ResizeObserver
let disposed = false
let generation = 0
let pendingBytes = 0
let backlog = false
const send = (value: unknown) => {
  if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(value))
}
function resize() {
  if (!props.active || !container.value?.clientWidth || !container.value.clientHeight) return
  fit.fit()
  send({ type: 'resize', cols: term.cols, rows: term.rows })
}
function connect() {
  const current = ++generation
  socket?.close()
  status.value = '连接中'
  connected.value = false
  const url = new URL('/api/terminal', window.location.href)
  url.protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  url.searchParams.set('repo', props.repo)
  const connection = new WebSocket(url)
  connection.binaryType = 'arraybuffer'
  socket = connection
  connection.onopen = () => {
    if (current !== generation || disposed) return
    status.value = '已连接'
    connected.value = true
    resize()
    if (props.active) term.focus()
  }
  connection.onmessage = event => {
    if (current !== generation || disposed) return
    if (event.data instanceof ArrayBuffer) {
      const data = new Uint8Array(event.data)
      pendingBytes += data.byteLength
      if (pendingBytes > 1024 * 1024 && !backlog) { backlog = true; send({ type: 'pause' }) }
      term.write(data, () => {
        if (current !== generation || disposed) return
        pendingBytes -= data.byteLength
        if (backlog && pendingBytes < 256 * 1024) { backlog = false; send({ type: 'resume' }) }
      })
      return
    }
    try {
      const message = JSON.parse(event.data)
      if (message.type === 'repo') emit('repo', message.repo)
      if (message.type === 'error') { status.value = message.message; term.writeln(`\r\n${message.message}`) }
      if (message.type === 'exit') { status.value = '会话已结束'; connected.value = false }
    } catch { /* Ignore messages that are not protocol events. */ }
  }
  connection.onclose = () => {
    if (current !== generation || disposed) return
    connected.value = false
    if (status.value === '已连接' || status.value === '连接中') status.value = '连接已关闭'
    term.writeln('\r\n\x1b[90m[终端连接已关闭；可点击「新建会话」]\x1b[0m')
  }
  connection.onerror = () => {
    if (current === generation && !disposed) status.value = '连接失败，请检查服务是否支持 PTY'
  }
}
function restart() {
  if (connected.value && !window.confirm('结束当前 shell 及其中的程序，并新建终端会话？')) return
  term.reset()
  pendingBytes = 0
  backlog = false
  connect()
}
onMounted(() => {
  term = new Terminal({ cursorBlink: true, fontSize: props.fontSize, fontFamily: props.fontFamily, scrollback: 5000,
    theme: { background: '#18232c', foreground: '#d9e2e8', cursor: '#91dbb2', selectionBackground: '#415d6b' } })
  fit = new FitAddon()
  term.loadAddon(fit)
  term.open(container.value!)
  term.onData(data => send({ type: 'input', data }))
  term.onBinary(data => { if (socket?.readyState === WebSocket.OPEN) socket.send(Uint8Array.from(data, char => char.charCodeAt(0))) })
  observer = new ResizeObserver(resize)
  observer.observe(container.value!)
  resize()
  connect()
})
watch(fullscreen, async () => { await nextTick(); resize(); term?.focus() })
watch(() => props.active, async active => { if (active) { await nextTick(); resize(); term?.focus() } })
watch(() => [props.fontSize, props.fontFamily], async () => {
  if (!term) return
  term.options.fontSize = props.fontSize
  term.options.fontFamily = props.fontFamily
  await nextTick()
  resize()
})
onUnmounted(() => {
  disposed = true
  generation++
  observer?.disconnect()
  socket?.close()
  term?.dispose()
})
</script>

<template>
  <div class="terminal-panel" :class="{ fullscreen }">
    <div class="terminal-toolbar"><strong>终端 <small>{{ status }}</small></strong><div class="terminal-tools"><button class="outline-button" @click="fullscreen = !fullscreen">{{ fullscreen ? '退出全屏' : '全屏' }}</button><button class="outline-button" :disabled="!connected" @click="send({ type: 'input', data: '\x03' }); term.focus()">Ctrl+C</button><button class="outline-button" @click="term.clear(); term.focus()">清屏</button><button class="outline-button" @click="restart">新建会话</button></div></div>
    <div class="terminal-path" :title="repo">{{ repo }}</div>
    <div ref="container" class="pty-terminal" aria-label="交互式终端"></div>
    <div class="terminal-foot">可直接运行 ls、git、codex · 切换标签和仓库保留会话；刷新或关闭页面会结束会话。</div>
  </div>
</template>
