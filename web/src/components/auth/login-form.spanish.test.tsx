// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { afterEach, expect, test } from 'bun:test'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { I18nProvider } from '@/components/providers/I18nProvider'
import { LOCALE_STORAGE_KEY } from '@/lib/i18n'
import { installBrowserWindow } from '@/test/dom'
import { LoginForm, providerButtonLabel } from './login-form'
import { es } from '@/lib/i18n'

installBrowserWindow()
afterEach(() => {
  cleanup()
  localStorage.clear()
})

test('in Spanish, labels, actions and validation messages are translated', async () => {
  localStorage.setItem(LOCALE_STORAGE_KEY, 'es')
  render(
    <I18nProvider>
      <MemoryRouter>
        <LoginForm onSubmit={async () => {}} isLoading={false} />
      </MemoryRouter>
    </I18nProvider>
  )

  expect(screen.getByText('Correo electrónico')).toBeDefined()
  expect(
    screen.getByRole('link', { name: '¿Olvidaste tu contraseña?' })
  ).toBeDefined()
  const submit = screen.getByRole('button', { name: 'Iniciar sesión' })

  await act(async () => {
    fireEvent.click(submit)
  })
  expect(
    await screen.findByText('Introduce un correo electrónico válido')
  ).toBeDefined()
  expect(screen.getByText('La contraseña es obligatoria')).toBeDefined()
})

test('SSO button labels follow the language', () => {
  expect(providerButtonLabel({ slug: 'okta', name: 'Okta' }, es)).toBe(
    'Iniciar sesión con Okta'
  )
  expect(providerButtonLabel({ slug: 'okta', name: 'Okta' })).toBe(
    'Sign in with Okta'
  )
})
