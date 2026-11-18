// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { Monitor, Moon, Sun } from 'lucide-react'
import type { Messages } from '@/lib/i18n'

/** The appearance choices next-themes understands, with their label and icon. */
export const THEME_OPTIONS = [
  { value: 'light', labelKey: 'light', icon: Sun },
  { value: 'dark', labelKey: 'dark', icon: Moon },
  { value: 'system', labelKey: 'system', icon: Monitor },
] as const satisfies readonly {
  value: string
  labelKey: keyof Messages['preferences']
  icon: unknown
}[]

export type ThemeOption = (typeof THEME_OPTIONS)[number]

/** The option for next-themes' current value; System when unset or unknown. */
export function themeOptionFor(theme: string | undefined): ThemeOption {
  return (
    THEME_OPTIONS.find((option) => option.value === theme) ?? THEME_OPTIONS[2]
  )
}
