// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, test } from 'bun:test'
import { detectLocale, en, es, format, formatParts, navTitle } from './index'

describe('detectLocale', () => {
  test('a saved choice wins over the browser', () => {
    expect(detectLocale('es', ['en-US'])).toBe('es')
  })

  test('falls back to the first supported browser language, ignoring the region', () => {
    expect(detectLocale(null, ['fr-FR', 'es-MX', 'en'])).toBe('es')
  })

  test('ignores an unsupported saved value and defaults to English', () => {
    expect(detectLocale('klingon', ['de-DE'])).toBe('en')
  })
})

describe('format', () => {
  test('fills placeholders and leaves unknown ones as written', () => {
    expect(format('Sign in with {name} {other}', { name: 'Okta' })).toBe(
      'Sign in with Okta {other}'
    )
  })

  test('formatParts keeps a node value in place', () => {
    const node = { type: 'span' } as unknown as string
    expect(formatParts('Sent to {email}.', { email: node })).toEqual([
      'Sent to ',
      node,
      '.',
    ])
  })
})

describe('navTitle', () => {
  test('translates a known title and keeps an unknown one in English', () => {
    expect(navTitle(es, 'Projects')).toBe('Proyectos')
    expect(navTitle(es, 'Some new page')).toBe('Some new page')
    expect(navTitle(en, 'Projects')).toBe('Projects')
  })
})

describe('messages', () => {
  // The Messages type already enforces this at build time; this keeps the
  // guarantee visible if someone loosens the type.
  const keys = (value: unknown, prefix = ''): string[] =>
    typeof value === 'string'
      ? [prefix]
      : Object.entries(value as Record<string, unknown>).flatMap(([k, v]) =>
          keys(v, prefix ? `${prefix}.${k}` : k)
        )
  test('Spanish has exactly the English keys', () => {
    const withoutNav = (m: typeof en | typeof es) => ({ ...m, navTitles: {} })
    expect(keys(withoutNav(es)).sort()).toEqual(keys(withoutNav(en)).sort())
  })
})
