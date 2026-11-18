// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { afterEach, expect, mock, test } from 'bun:test'
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react'
import type { DeploymentResponse } from '@/api/client'
import { installBrowserWindow } from '@/test/dom'
import { SecondaryActions } from './DeploymentDetails'

installBrowserWindow()
afterEach(cleanup)

function renderActions(deployment: Partial<DeploymentResponse>) {
  const actions = {
    onRedeploy: mock(() => {}),
    onCancel: mock(() => {}),
    onPause: mock(() => {}),
    onResume: mock(() => {}),
    onRollback: mock(() => {}),
    cancelPending: false,
    pausePending: false,
    resumePending: false,
    rollbackPending: false,
  }
  render(
    <SecondaryActions
      deployment={
        { id: 42, is_current: false, ...deployment } as DeploymentResponse
      }
      actions={actions}
    />
  )
  return actions
}

async function chooseFromMenu(item: string): Promise<HTMLElement> {
  await act(async () => {
    fireEvent.pointerDown(
      screen.getByRole('button', { name: 'More actions' }),
      {
        button: 0,
        ctrlKey: false,
      }
    )
  })
  await act(async () => {
    fireEvent.click(await screen.findByRole('menuitem', { name: item }))
  })
  return screen.findByRole('alertdialog')
}

function dialogButton(dialog: HTMLElement, label: string): HTMLButtonElement {
  const found = Array.from(dialog.querySelectorAll('button')).find(
    (button) => button.textContent === label
  )
  if (!found) throw new Error(`no "${label}" button in the dialog`)
  return found
}

const cases = [
  {
    item: 'Cancel deployment',
    deployment: { status: 'running' },
    handler: 'onCancel',
    confirm: 'Cancel deployment',
  },
  {
    item: 'Pause deployment',
    deployment: { status: 'completed', is_current: true },
    handler: 'onPause',
    confirm: 'Pause deployment',
  },
  {
    item: 'Rollback to this',
    deployment: { status: 'completed', is_current: false },
    handler: 'onRollback',
    confirm: 'Roll back',
  },
] as const

for (const { item, deployment, handler, confirm } of cases) {
  test(`"${item}" runs only after the user confirms`, async () => {
    const actions = renderActions(deployment)

    const dialog = await chooseFromMenu(item)
    expect(dialog.textContent).toContain('#42')
    expect(actions[handler]).not.toHaveBeenCalled()

    await act(async () => {
      fireEvent.click(dialogButton(dialog, confirm))
    })
    expect(actions[handler]).toHaveBeenCalledTimes(1)
  })
}

test('dismissing the confirmation sends nothing', async () => {
  const actions = renderActions({ status: 'completed', is_current: true })

  const dialog = await chooseFromMenu('Pause deployment')
  await act(async () => {
    fireEvent.click(dialogButton(dialog, 'Cancel'))
  })

  await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
  expect(actions.onPause).not.toHaveBeenCalled()
})
