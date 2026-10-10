// One transport for browser HTTP and the Wails-managed local service.
type DesktopConnection = { baseURL: string; token: string }
type DesktopBridge = { Connection(): Promise<DesktopConnection>; ChooseDirectory(): Promise<string> }
declare global {
  interface Window { go?: { main?: { DesktopApp?: DesktopBridge } } }
}
let connection: DesktopConnection | undefined
export const isDesktop = () => Boolean(window.go?.main?.DesktopApp)
export async function initializeBackend() {
  const bridge = window.go?.main?.DesktopApp
  if (bridge) connection = await bridge.Connection()
}
export function backendURL(path: string): string {
  if (!connection) return path
  const url = new URL(path, connection.baseURL)
  url.searchParams.set('desktopToken', connection.token)
  return url.toString()
}
export function backendHeaders(): Record<string, string> {
  return connection ? { 'X-MulitGit-Token': connection.token } : {}
}
export async function chooseDirectory(): Promise<string | null> {
  const bridge = window.go?.main?.DesktopApp
  return bridge ? bridge.ChooseDirectory() : window.prompt('输入要监控的本地目录绝对路径')
}
export function terminalURL(repo: string): URL {
  const url = new URL(backendURL(`/api/terminal?repo=${encodeURIComponent(repo)}`), window.location.href)
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  return url
}
