// SPDX-FileCopyrightText: 2024-2026 Temps Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Every change to a project's edge rules is kept as a numbered revision.
// Pick one to see what restoring it would change, then restore it.

import { Button } from '@/components/ui/button'
import { ConfirmDialog } from '@/components/ui/confirm-dialog'
import { ReadFailure } from '@/components/ui/read-failure'
import { ResponsivePagination } from '@/components/ui/responsive-pagination'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'
import { useQuery } from '@tanstack/react-query'
import { formatDistanceToNow } from 'date-fns'
import { History } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { describeChange, restoreChanges } from './history'
import {
  edgeRuleRevisionOptions,
  edgeRuleRevisionsOptions,
  useRestoreEdgeRuleRevision,
} from './store'
import type { EdgeRule } from './types'

const PER_PAGE = 20

interface RevisionHistorySheetProps {
  projectId: number
  open: boolean
  onOpenChange: (open: boolean) => void
  /** The rules in force, to compare a revision with. */
  current: EdgeRule[]
}

export function RevisionHistorySheet({
  projectId,
  open,
  onOpenChange,
  current,
}: RevisionHistorySheetProps) {
  const [page, setPage] = useState(1)
  const [selected, setSelected] = useState<number | null>(null)
  const [confirming, setConfirming] = useState(false)
  const listQuery = useQuery({
    ...edgeRuleRevisionsOptions(projectId, page, PER_PAGE),
    enabled: open,
  })
  const detailQuery = useQuery({
    ...edgeRuleRevisionOptions(projectId, selected ?? 0),
    enabled: open && selected !== null,
  })
  const restore = useRestoreEdgeRuleRevision(projectId)

  const revisions = listQuery.data?.revisions ?? []
  const total = listQuery.data?.total ?? 0
  // The newest revision is what is in force; restoring it changes nothing.
  const latest = page === 1 ? revisions[0]?.number : undefined
  const changes = detailQuery.data
    ? restoreChanges(detailQuery.data.rules, current)
    : []

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="w-full overflow-y-auto sm:max-w-xl">
        <SheetHeader>
          <SheetTitle>Rule history</SheetTitle>
          <SheetDescription>
            The last {listQuery.data?.max_revisions ?? 100} changes are kept.
          </SheetDescription>
        </SheetHeader>

        <div className="space-y-4 px-4 pb-6">
          {listQuery.isError ? (
            <ReadFailure
              resource="Rule history"
              error={listQuery.error}
              onRetry={() => void listQuery.refetch()}
              retrying={listQuery.isFetching}
            />
          ) : listQuery.isPending ? (
            <div className="space-y-2">
              {[0, 1, 2].map((i) => (
                <Skeleton key={i} className="h-12 w-full rounded-md" />
              ))}
            </div>
          ) : revisions.length === 0 ? (
            <p className="rounded-md border border-dashed p-4 text-sm text-muted-foreground">
              No changes yet. Each change you make to the rules will be listed
              here and can be restored.
            </p>
          ) : (
            <ol className="divide-y rounded-lg border">
              {revisions.map((revision) => (
                <li key={revision.number}>
                  <button
                    type="button"
                    onClick={() => setSelected(revision.number)}
                    aria-pressed={selected === revision.number}
                    className={cn(
                      'flex w-full items-start justify-between gap-3 px-3 py-2 text-left text-sm hover:bg-muted/60',
                      selected === revision.number && 'bg-muted/60'
                    )}
                  >
                    <span className="min-w-0">
                      <span className="block [overflow-wrap:anywhere]">
                        <span className="text-muted-foreground">
                          #{revision.number}
                        </span>{' '}
                        {revision.change}
                      </span>
                      <span className="block text-xs text-muted-foreground">
                        {revision.created_by_name ?? 'System'} ·{' '}
                        {formatDistanceToNow(new Date(revision.created_at), {
                          addSuffix: true,
                        })}{' '}
                        · {revision.rule_count}{' '}
                        {revision.rule_count === 1 ? 'rule' : 'rules'}
                      </span>
                    </span>
                    {revision.number === latest && (
                      <span className="shrink-0 text-xs text-muted-foreground">
                        In force
                      </span>
                    )}
                  </button>
                </li>
              ))}
            </ol>
          )}

          {total > PER_PAGE && (
            <ResponsivePagination
              page={page}
              pageSize={PER_PAGE}
              total={total}
              totalPages={Math.max(1, Math.ceil(total / PER_PAGE))}
              onPageChange={setPage}
            />
          )}

          {selected !== null && (
            <div className="space-y-3 rounded-lg border p-3">
              <h3 className="text-sm font-medium">
                Restoring #{selected} would
              </h3>
              {detailQuery.isError ? (
                <ReadFailure
                  resource={`Revision #${selected}`}
                  error={detailQuery.error}
                  onRetry={() => void detailQuery.refetch()}
                  retrying={detailQuery.isFetching}
                />
              ) : detailQuery.isPending ? (
                <Skeleton className="h-16 w-full rounded-md" />
              ) : changes.length === 0 ? (
                <p className="text-sm text-muted-foreground">
                  Change nothing: these are the rules in force.
                </p>
              ) : (
                <ul className="list-disc space-y-1 pl-5 text-sm">
                  {changes.map((change) => (
                    <li key={`${change.kind}-${change.name}`}>
                      {describeChange(change)}
                    </li>
                  ))}
                </ul>
              )}
              <Button
                size="sm"
                disabled={
                  changes.length === 0 ||
                  detailQuery.isPending ||
                  restore.isPending
                }
                onClick={() => setConfirming(true)}
              >
                <History />
                Restore #{selected}
              </Button>
            </div>
          )}
        </div>

        <ConfirmDialog
          open={confirming}
          onOpenChange={setConfirming}
          title={`Restore revision #${selected ?? ''}?`}
          description="Every proxy enforces the restored rules within seconds. The current rules stay in the history, so you can restore them again."
          confirmLabel="Restore"
          onConfirm={() => {
            if (selected === null) return
            const number = selected
            restore.mutate(number, {
              onSuccess: () => {
                toast.success(`Restored revision #${number}`)
                setConfirming(false)
                setSelected(null)
                setPage(1)
              },
              onError: () => setConfirming(false),
            })
          }}
        />
      </SheetContent>
    </Sheet>
  )
}
