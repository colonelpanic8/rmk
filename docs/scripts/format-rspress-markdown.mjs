#!/usr/bin/env node

import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import prettier from 'prettier'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const packageRoot = path.resolve(__dirname, '..')
const docsRoot = path.join(packageRoot, 'docs/main')
const check = process.argv.includes('--check')

const markdownExtensions = new Set(['.md', '.mdx'])

function listMarkdownFiles(dir) {
  const files = []

  for (const entry of readdirSync(dir)) {
    const fullPath = path.join(dir, entry)
    const stats = statSync(fullPath)

    if (stats.isDirectory()) {
      files.push(...listMarkdownFiles(fullPath))
    } else if (stats.isFile() && markdownExtensions.has(path.extname(entry))) {
      files.push(fullPath)
    }
  }

  return files
}

function getCodeFence(line, activeFence) {
  const match = line.match(/^ {0,3}(`{3,}|~{3,})/)

  if (!match) {
    return null
  }

  const marker = match[1]
  const fence = {
    char: marker[0],
    length: marker.length
  }

  if (!activeFence) {
    return fence
  }

  if (fence.char === activeFence.char && fence.length >= activeFence.length) {
    return false
  }

  return null
}

function isContainerClose(line) {
  return /^(\s*):{3,}\s*$/.test(line)
}

function normalizeContainers(source) {
  const hasFinalNewline = source.endsWith('\n')
  const lines = source.replace(/\r\n/g, '\n').split('\n')

  if (hasFinalNewline) {
    lines.pop()
  }

  const output = []
  let activeCodeFence = null

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index]
    const fence = getCodeFence(line, activeCodeFence)

    if (fence !== null) {
      activeCodeFence = fence === false ? null : fence
      output.push(line.trimEnd())
      continue
    }

    if (!activeCodeFence) {
      const inlineContainer = line.match(/^(\s*)(:{3,})\s+([A-Za-z][\w-]*)\s+(.+?)\s+\2\s*$/)

      if (inlineContainer) {
        const [, indent, marker, type, body] = inlineContainer
        output.push(`${indent}${marker} ${type}`)
        output.push('')
        output.push(body.trim())
        output.push('')
        output.push(`${indent}${marker}`)
        continue
      }

      const opener = line.match(/^(\s*)(:{3,})\s+(.+\S)\s*$/)

      if (opener) {
        output.push(line.trimEnd())

        const nextLine = lines[index + 1] ?? ''
        if (nextLine.trim() !== '' && !isContainerClose(nextLine)) {
          output.push('')
        }

        continue
      }

      if (isContainerClose(line)) {
        if (output.length > 0 && output[output.length - 1].trim() !== '') {
          output.push('')
        }

        output.push(line.trimEnd())
        continue
      }
    }

    output.push(line.trimEnd())
  }

  return `${output.join('\n')}${hasFinalNewline ? '\n' : ''}`
}

function compactContainers(source) {
  const hasFinalNewline = source.endsWith('\n')
  const lines = source.replace(/\r\n/g, '\n').split('\n')

  if (hasFinalNewline) {
    lines.pop()
  }

  const output = []
  let activeCodeFence = null

  for (const line of lines) {
    const fence = getCodeFence(line, activeCodeFence)

    if (fence !== null) {
      activeCodeFence = fence === false ? null : fence
      output.push(line)
      continue
    }

    if (!activeCodeFence && isContainerClose(line) && output[output.length - 1]?.trim() === '') {
      output.pop()
    }

    output.push(line)

    if (!activeCodeFence && /^(\s*)(:{3,})\s+(.+\S)\s*$/.test(line)) {
      continue
    }
  }

  const compacted = []
  activeCodeFence = null

  for (let index = 0; index < output.length; index += 1) {
    const line = output[index]
    const fence = getCodeFence(line, activeCodeFence)

    if (fence !== null) {
      activeCodeFence = fence === false ? null : fence
      compacted.push(line)
      continue
    }

    compacted.push(line)

    if (
      !activeCodeFence &&
      /^(\s*)(:{3,})\s+(.+\S)\s*$/.test(line) &&
      output[index + 1]?.trim() === ''
    ) {
      index += 1
    }
  }

  return `${compacted.join('\n')}${hasFinalNewline ? '\n' : ''}`
}

async function formatMarkdown(file, source) {
  const normalized = normalizeContainers(source)
  const config = (await prettier.resolveConfig(file)) ?? {}
  const formatted = await prettier.format(normalized, {
    ...config,
    filepath: file
  })

  return compactContainers(formatted)
}

const changedFiles = []

for (const file of listMarkdownFiles(docsRoot)) {
  const source = readFileSync(file, 'utf8')
  const formatted = await formatMarkdown(file, source)

  if (formatted !== source) {
    changedFiles.push(path.relative(packageRoot, file))

    if (!check) {
      writeFileSync(file, formatted)
    }
  }
}

if (check && changedFiles.length > 0) {
  console.error('Rspress Markdown formatting is needed:')
  for (const file of changedFiles) {
    console.error(`  ${file}`)
  }
  process.exit(1)
}
