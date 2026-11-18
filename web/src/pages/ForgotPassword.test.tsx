// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { afterEach, expect, test } from 'bun:test'
import { cleanup, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router'
import { I18nProvider } from '@/components/providers/I18nProvider'
import { LOCALE_STORAGE_KEY } from '@/lib/i18n'
import { installBrowserWindow } from '@/test/dom'
import { jsonResponse, mockApi } from '@/test/fetch-mock'
import { ForgotPassword, RESET_ADMIN_PASSWORD_COMMAND } from './ForgotPassword'

installBrowserWindow()
afterEach(() => {
  cleanup()
  localStorage.clear()
})

function renderPage(passwordResetAvailable: boolean) {
  mockApi({
    'GET /auth/email-status': () =>
      jsonResponse({
        password_reset_available: passwordResetAvailable,
        oidc_providers: [],
      }),
  })
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <I18nProvider>
      <QueryClientProvider client={queryClient}>
        <MemoryRouter>
          <ForgotPassword />
        </MemoryRouter>
      </QueryClientProvider>
    </I18nProvider>
  )
}

test('in Spanish the page is translated and offers language and appearance', async () => {
  localStorage.setItem(LOCALE_STORAGE_KEY, 'es')
  renderPage(false)

  await screen.findByText('Recuperación de contraseña no disponible')
  expect(screen.getByText('¿Olvidaste tu contraseña?')).toBeDefined()
  expect(screen.getByRole('button', { name: 'Copiar comando' })).toBeDefined()
  expect(screen.getByRole('button', { name: 'Idioma: Español' })).toBeDefined()
  expect(
    screen.getByRole('button', { name: 'Apariencia: Sistema' })
  ).toBeDefined()
})

test('without email, the page gives the terminal command that resets the admin password', async () => {
  renderPage(false)

  await screen.findByText('Password reset unavailable')
  expect(screen.getByText(RESET_ADMIN_PASSWORD_COMMAND)).toBeDefined()
  expect(screen.getByRole('button', { name: 'Copy command' })).toBeDefined()
  expect(document.body.textContent).not.toContain('Contact your administrator')
})

test('with email configured, the page offers the reset link and no command', async () => {
  renderPage(true)

  const submit = await screen.findByRole('button', { name: 'Send reset link' })
  await waitFor(() =>
    expect((submit as HTMLButtonElement).disabled).toBe(false)
  )
  expect(screen.queryByText(RESET_ADMIN_PASSWORD_COMMAND)).toBeNull()
})
