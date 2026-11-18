// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react'
import {
  detectLocale,
  en,
  LOCALE_STORAGE_KEY,
  LOCALES,
  type Locale,
  type Messages,
} from '@/lib/i18n'

interface I18nValue {
  locale: Locale
  /** The current language's messages: `t.auth.signIn`. */
  t: Messages
  setLocale: (locale: Locale) => void
}

// Without a provider (isolated components, tests) everything reads English.
const I18nContext = createContext<I18nValue>({
  locale: 'en',
  t: en,
  setLocale: () => {},
})

function readStoredLocale(): string | null {
  try {
    return localStorage.getItem(LOCALE_STORAGE_KEY)
  } catch {
    return null
  }
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(() =>
    detectLocale(
      readStoredLocale(),
      navigator.languages?.length ? navigator.languages : [navigator.language]
    )
  )

  // The page's language follows the text, for screen readers and spellcheck (WCAG 3.1.1).
  useEffect(() => {
    document.documentElement.lang = locale
  }, [locale])

  const setLocale = useCallback((next: Locale) => {
    setLocaleState(next)
    try {
      localStorage.setItem(LOCALE_STORAGE_KEY, next)
    } catch {
      // Storage can be blocked (private mode); the choice then lasts for this visit.
    }
  }, [])

  const value = useMemo(
    () => ({ locale, t: LOCALES[locale], setLocale }),
    [locale, setLocale]
  )
  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>
}

export function useI18n(): I18nValue {
  return useContext(I18nContext)
}
