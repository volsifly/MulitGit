export type RepositorySort = 'name' | 'created' | 'modified'

export interface TreeRepository {
  path: string
  name: string
  branch: string
  dirty: boolean
  ahead: number
  behind: number
  createdAt?: number
  lastModified: number
}

export interface RepositoryTreeNode {
  key: string
  name: string
  path: string
  rootIndex?: number
  repository?: TreeRepository
  children: RepositoryTreeNode[]
  createdAt: number
  lastModified: number
}

export function normalizeTreePath(path: string): string {
  return path.replaceAll('\\', '/').replace(/\/+$/, '') || '/'
}

// Compare path segments, so /work/app never includes /work/application.
function relativePath(root: string, path: string): string | null {
  if (root === path) return ''
  const prefix = root.endsWith('/') ? root : `${root}/`
  return path.startsWith(prefix) ? path.slice(prefix.length) : null
}

export function buildRepositoryTrees(roots: string[], repos: TreeRepository[], sort: RepositorySort): RepositoryTreeNode[] {
  const createNode = (root: string, path: string, name: string): RepositoryTreeNode => ({
    key: JSON.stringify([root, path]), name, path, children: [], createdAt: 0, lastModified: 0,
  })
  const trees = roots.map((watchRoot, rootIndex) => {
    const root = normalizeTreePath(watchRoot)
    const tree = createNode(root, root, root.split('/').filter(Boolean).slice(-2).join('/') || root)
    tree.rootIndex = rootIndex
    const nodes = new Map([[root, tree]])
    for (const repo of repos) {
      const path = normalizeTreePath(repo.path)
      const relative = relativePath(root, path)
      if (relative === null) continue
      let parent = tree
      let currentPath = root
      for (const segment of relative.split('/').filter(Boolean)) {
        currentPath = `${currentPath === '/' ? '' : currentPath}/${segment}`
        let child = nodes.get(currentPath)
        if (!child) {
          child = createNode(root, currentPath, segment)
          nodes.set(currentPath, child)
          parent.children.push(child)
        }
        parent = child
      }
      parent.repository = repo
    }
    return tree
  })
  const compare = (a: RepositoryTreeNode, b: RepositoryTreeNode): number => {
    const nameOrder = a.name.localeCompare(b.name, 'zh-CN', { numeric: true, sensitivity: 'base' }) || a.path.localeCompare(b.path)
    if (sort === 'name') return nameOrder
    return (sort === 'created' ? b.createdAt - a.createdAt : b.lastModified - a.lastModified) || nameOrder
  }
  const sortTree = (node: RepositoryTreeNode) => {
    node.createdAt = node.repository?.createdAt ?? 0
    node.lastModified = node.repository?.lastModified ?? 0
    for (const child of node.children) {
      sortTree(child)
      node.createdAt = Math.max(node.createdAt, child.createdAt)
      node.lastModified = Math.max(node.lastModified, child.lastModified)
    }
    node.children.sort(compare)
  }
  trees.forEach(sortTree)
  return trees.sort(compare)
}
