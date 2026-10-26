<script setup lang="ts">
import { computed } from 'vue'
import type { RepositoryTreeNode } from './repositoryTree'

const props = withDefaults(defineProps<{
  node: RepositoryTreeNode
  collapsed: ReadonlySet<string>
  selectedPath: string
  depth?: number
}>(), { depth: 0 })
const emit = defineEmits<{
  toggle: [key: string]
  select: [path: string]
  remove: [index: number]
}>()
const expanded = computed(() => !props.collapsed.has(props.node.key))
const isRoot = computed(() => props.node.rootIndex !== undefined)
const expandable = computed(() => isRoot.value || props.node.children.length > 0)
function activate() {
  if (props.node.repository) emit('select', props.node.repository.path)
  else emit('toggle', props.node.key)
}
</script>

<template>
  <div class="repository-tree-node" role="treeitem" :aria-level="depth + 1"
    :aria-expanded="expandable ? expanded : undefined" :data-tree-path="node.path">
    <div class="tree-item-row" :class="{ 'tree-root-row': isRoot, 'tree-repo-row': node.repository, active: node.repository && selectedPath === node.repository.path }"
      :style="{ paddingLeft: `${6 + depth * 16}px` }">
      <button v-if="expandable" class="disclosure tree-disclosure" :aria-label="`${expanded ? '折叠' : '展开'} ${node.name}`"
        :aria-expanded="expanded" @click="emit('toggle', node.key)">{{ expanded ? '⌄' : '›' }}</button>
      <span v-else class="tree-disclosure-spacer" aria-hidden="true"></span>
      <button class="tree-item-main" :title="node.repository?.path ?? node.path" @click="activate">
        <template v-if="node.repository">
          <span class="repo-indicator" :class="{ dirty: node.repository.dirty }"></span>
          <span class="repo-glyph" aria-hidden="true">⌘</span>
          <span class="repo-label"><b>{{ node.name }}</b><small>{{ node.repository.branch }}</small></span>
          <span v-if="node.repository.behind" class="ahead-badge" :title="`远端领先 ${node.repository.behind} 个提交，可拉取`">↓{{ node.repository.behind }}</span>
          <span v-if="node.repository.ahead" class="ahead-badge" :title="`${node.repository.ahead} 个本地提交尚未推送`">↑{{ node.repository.ahead }}</span>
        </template>
        <template v-else><span class="folder-icon" aria-hidden="true">▱</span><span class="tree-folder-label">{{ node.name }}</span></template>
      </button>
      <button v-if="isRoot" class="root-remove" :aria-label="`移除监控目录 ${node.path}`" title="移除监控目录"
        @click="emit('remove', node.rootIndex!)">×</button>
    </div>
    <div v-if="expanded && expandable" role="group">
      <RepositoryTreeItem v-for="child in node.children" :key="child.key" :node="child" :collapsed="collapsed"
        :selected-path="selectedPath" :depth="depth + 1" @toggle="emit('toggle', $event)" @select="emit('select', $event)" />
      <div v-if="isRoot && !node.repository && !node.children.length" class="empty-root">暂未发现 Git 仓库</div>
    </div>
  </div>
</template>
