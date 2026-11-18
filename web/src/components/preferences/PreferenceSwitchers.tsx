// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { Check, Languages } from 'lucide-react'
import { useTheme } from 'next-themes'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useI18n } from '@/components/providers/I18nProvider'
import { LOCALE_NAMES, LOCALES, type Locale } from '@/lib/i18n'
import { THEME_OPTIONS, themeOptionFor } from './theme-options'

const LOCALE_CODES = Object.keys(LOCALES) as Locale[]

/** Light, dark or system appearance, from anywhere (no account needed). */
export function ThemeSwitcher() {
  const { theme, setTheme } = useTheme()
  const { t } = useI18n()
  const current = themeOptionFor(theme)
  const currentLabel = t.preferences[current.labelKey]
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          aria-label={`${t.preferences.appearance}: ${currentLabel}`}
        >
          <current.icon className="h-4 w-4" aria-hidden="true" />
          <span className="max-sm:sr-only">{currentLabel}</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {THEME_OPTIONS.map((option) => (
          <DropdownMenuItem
            key={option.value}
            onSelect={() => setTheme(option.value)}
          >
            <option.icon className="mr-2 h-4 w-4" aria-hidden="true" />
            {t.preferences[option.labelKey]}
            {theme === option.value && (
              <Check className="ml-auto h-4 w-4" aria-hidden="true" />
            )}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** The console's language; the choice is kept in this browser. */
export function LanguageSwitcher() {
  const { locale, setLocale, t } = useI18n()
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          aria-label={`${t.preferences.language}: ${LOCALE_NAMES[locale]}`}
        >
          <Languages className="h-4 w-4" aria-hidden="true" />
          <span className="max-sm:sr-only">{LOCALE_NAMES[locale]}</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {LOCALE_CODES.map((code) => (
          <DropdownMenuItem
            key={code}
            lang={code}
            onSelect={() => setLocale(code)}
          >
            {LOCALE_NAMES[code]}
            {locale === code && (
              <Check className="ml-auto h-4 w-4" aria-hidden="true" />
            )}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** Appearance and language in the corner of the signed-out pages. */
export function AuthPreferences() {
  return (
    <div className="absolute right-4 top-4 flex items-center gap-1">
      <LanguageSwitcher />
      <ThemeSwitcher />
    </div>
  )
}
