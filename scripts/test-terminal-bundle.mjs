import assert from 'node:assert/strict'
import { readFileSync, readdirSync } from 'node:fs'
import { test } from 'node:test'
import vm from 'node:vm'
import ts from 'typescript'

// Exercise the shipped parser, since the development dependency works correctly.
const assets = new URL('../internal/webui/dist/assets/', import.meta.url)
const bundle = readdirSync(assets).find(name => /^RepoTerminal-.*\.js$/.test(name))
assert.ok(bundle, 'Build the production frontend before running this regression check')
const source = ts.createSourceFile(bundle, readFileSync(new URL(bundle, assets), 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.JS)
let method
function visit(node) {
  if (ts.isMethodDeclaration(node) && node.name.getText(source) === 'requestMode') method = node.getText(source)
  ts.forEachChild(node, visit)
}
visit(source)
assert.ok(method, 'Production xterm parser must contain requestMode')
// ESC is the only module constant used by this protocol handler.
const handler = vm.runInNewContext(`'use strict'; ({${method.replace(/\b\w+\.ESC\b/g, JSON.stringify('\x1b'))}})`)
const responses = []
const context = {
  _coreService: { decPrivateModes: { bracketedPasteMode: true, synchronizedOutput: false }, modes: { insertMode: false }, triggerDataEvent: value => responses.push(value) },
  _coreMouseService: { activeProtocol: 'NONE', activeEncoding: 'DEFAULT' },
  _bufferService: { buffers: { active: {}, alt: {} }, cols: 80 },
  _optionsService: { rawOptions: { windowOptions: {}, convertEol: false } },
}

test('production parser answers TTT startup capability queries without stopping output', () => {
  for (const mode of [2048, 1000, 1006, 9001, 2004, 2026]) {
    assert.equal(handler.requestMode.call(context, { params: [mode] }, false), true)
  }
  assert.deepEqual(responses.splice(0), ['\x1b[?2048;0$y', '\x1b[?1000;2$y', '\x1b[?1006;2$y', '\x1b[?9001;0$y', '\x1b[?2004;1$y', '\x1b[?2026;2$y'])
})

test('production parser still answers standard ANSI mode queries', () => {
  assert.equal(handler.requestMode.call(context, { params: [4] }, true), true)
  assert.deepEqual(responses.splice(0), ['\x1b[4;2$y'])
})
