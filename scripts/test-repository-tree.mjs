import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import ts from 'typescript'

const source = readFileSync(new URL('../web/src/repositoryTree.ts', import.meta.url), 'utf8')
const { outputText } = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 } })
const { buildRepositoryTrees } = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`)
const repo = (path, modified = 1, created = 1) => ({ path, name: path.split('/').at(-1), branch: 'main', dirty: false, ahead: 0, behind: 0, lastModified: modified, createdAt: created })

test('preserves all ancestors and keeps sibling paths separate', () => {
  const [root] = buildRepositoryTrees(['/work'], [repo('/work/team/service/api'), repo('/work/team/site'), repo('/work/team2/api'), repo('/worker/unrelated')], 'name')
  assert.deepEqual(root.children.map(node => node.name), ['team', 'team2'])
  assert.equal(root.children[0].children[0].name, 'service')
  assert.equal(root.children[0].children[0].children[0].repository.path, '/work/team/service/api')
  assert.equal(root.children[1].children[0].repository.path, '/work/team2/api')
})

test('a monitored repository occupies the root and submodules retain their ancestors', () => {
  const [root] = buildRepositoryTrees(['/work/app/'], [repo('/work/app/vendor/sdk'), repo('/work/app')], 'name')
  assert.equal(root.repository.path, '/work/app')
  assert.equal(root.children[0].name, 'vendor')
  assert.equal(root.children[0].children[0].repository.path, '/work/app/vendor/sdk')
  assert.equal(root.children.length, 1)
})

test('sorts folders by their newest descendant and keeps original root deletion indices', () => {
  const roots = ['/empty', '/work']
  const repos = [repo('/work/z/api', 100, 1), repo('/work/a/site', 2, 200)]
  const modified = buildRepositoryTrees(roots, repos, 'modified')
  assert.equal(modified[0].rootIndex, 1)
  assert.deepEqual(modified[0].children.map(node => node.name), ['z', 'a'])
  assert.deepEqual(buildRepositoryTrees(roots, repos, 'created')[0].children.map(node => node.name), ['a', 'z'])
  assert.deepEqual(buildRepositoryTrees(roots, repos, 'name').map(node => node.path), ['/empty', '/work'])
})

test('normalizes Windows separators, drive roots and filesystem roots', () => {
  const [windows] = buildRepositoryTrees(['C:\\'], [repo('C:\\team\\app'), repo('D:\\other')], 'name')
  assert.equal(windows.children[0].name, 'team')
  assert.equal(windows.children[0].children[0].name, 'app')
  const [unix] = buildRepositoryTrees(['/'], [repo('/team/app')], 'name')
  assert.equal(unix.children[0].path, '/team')
})

test('folder keys survive sorting and refresh, with independent keys for overlapping roots', () => {
  const roots = ['/work', '/work/team']
  const repos = [repo('/work/team/app')]
  const initial = buildRepositoryTrees(roots, repos, 'name')
  const refreshed = buildRepositoryTrees(roots, [...repos, repo('/work/new/repo', 100)], 'modified')
  const parent = initial.find(root => root.path === '/work')
  const nested = initial.find(root => root.path === '/work/team')
  assert.notEqual(parent.children[0].key, nested.key)
  assert.equal(refreshed.find(root => root.path === '/work').children.find(node => node.name === 'team').key, parent.children[0].key)
})
