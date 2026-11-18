// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { afterEach, expect, test } from 'bun:test'
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react'
import { ThemeProvider } from '@/components/providers/ThemeProvider'
import { I18nProvider, useI18n } from '@/components/providers/I18nProvider'
import { LOCALE_STORAGE_KEY } from '@/lib/i18n'
import { installBrowserWindow } from '@/test/dom'
import { LanguageSwitcher, ThemeSwitcher } from './PreferenceSwitchers'

installBrowserWindow()
afterEach(() => {
  cleanup()
  localStorage.clear()
  document.documentElement.lang = ''
  document.documentElement.className = ''
})

function SignInLabel() {
  const { t } = useI18n()
  return <p>{t.auth.signIn}</p>
}

async function choose(trigger: HTMLElement, item: string) {
  await act(async () => {
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false })
  })
  await act(async () => {
    fireEvent.click(await screen.findByRole('menuitem', { name: item }))
  })
}

test('choosing Español translates the console, sets <html lang> and remembers it', async () => {
  localStorage.setItem(LOCALE_STORAGE_KEY, 'en')
  render(
    <I18nProvider>
      <LanguageSwitcher />
      <SignInLabel />
    </I18nProvider>
  )
  expect(screen.getByText('Sign in')).toBeDefined()

  await choose(
    screen.getByRole('button', { name: 'Language: English' }),
    'Español'
  )

  expect(screen.getByText('Iniciar sesión')).toBeDefined()
  expect(document.documentElement.lang).toBe('es')
  expect(localStorage.getItem(LOCALE_STORAGE_KEY)).toBe('es')
  expect(screen.getByRole('button', { name: 'Idioma: Español' })).toBeDefined()
})

test('choosing Dark switches the page to the dark theme', async () => {
  render(
    <ThemeProvider attribute="class" defaultTheme="light" enableSystem>
      <ThemeSwitcher />
    </ThemeProvider>
  )

  await choose(
    screen.getByRole('button', { name: 'Appearance: Light' }),
    'Dark'
  )

  await waitFor(() =>
    expect(document.documentElement.classList.contains('dark')).toBe(true)
  )
  expect(screen.getByRole('button', { name: 'Appearance: Dark' })).toBeDefined()
})
