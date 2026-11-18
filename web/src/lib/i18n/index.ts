// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import type { ReactNode } from 'react'
import { en, es, type Messages } from './messages'

export { en, es, type Messages }

export const LOCALES = { en, es } as const satisfies Record<string, Messages>
export type Locale = keyof typeof LOCALES

/** Each language named in itself, so a reader can find their own. */
export const LOCALE_NAMES: Record<Locale, string> = {
  en: 'English',
  es: 'Español',
}

export const LOCALE_STORAGE_KEY = 'temps-locale'

const isLocale = (value: string): value is Locale => value in LOCALES

/**
 * The saved choice wins, then the browser's languages in order ("es-MX"
 * matches "es"), then English.
 */
export function detectLocale(
  stored: string | null,
  browserLanguages: readonly string[]
): Locale {
  for (const tag of [stored, ...browserLanguages]) {
    const base = tag?.toLowerCase().split('-')[0]
    if (base && isLocale(base)) return base
  }
  return 'en'
}

/** Replaces `{name}` placeholders; an unknown placeholder is left as written. */
export function format(
  message: string,
  values: Record<string, string | number>
): string {
  return message.replace(/\{(\w+)\}/g, (match, key: string) =>
    key in values ? String(values[key]) : match
  )
}

/** Like `format`, but a value may be a React node (a styled name, a link). */
export function formatParts(
  message: string,
  values: Record<string, ReactNode>
): ReactNode[] {
  return message
    .split(/(\{\w+\})/)
    .filter((part) => part !== '')
    .map((part) => {
      const key = /^\{(\w+)\}$/.exec(part)?.[1]
      return key !== undefined && key in values ? values[key] : part
    })
}

/** A primary-navigation title in the current language, English when untranslated. */
export function navTitle(messages: Messages, title: string): string {
  return messages.navTitles[title] ?? title
}
