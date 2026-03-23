import { getStack } from '@/api/stacks'
import { StackDetailView } from '@/components/stacks/StackDetailView'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { usePageTitle } from '@/hooks/usePageTitle'
import { useQuery } from '@tanstack/react-query'
import { useEffect } from 'react'
import { useParams } from 'react-router-dom'

export function StackDetail() {
  const { id } = useParams<{ id: string }>()
  const { setBreadcrumbs } = useBreadcrumbs()

  const { data: stack } = useQuery({
    queryKey: ['stacks', parseInt(id!)],
    queryFn: async () => {
      const { data } = await getStack(parseInt(id!))
      return data
    },
    enabled: !!id,
  })

  useEffect(() => {
    setBreadcrumbs([
      { label: 'Stacks', href: '/stacks' },
      { label: stack?.name || `Stack #${id}` },
    ])
  }, [setBreadcrumbs, id, stack?.name])

  usePageTitle(stack?.name || `Stack #${id}`)

  if (!id) return null

  return (
    <div className="flex-1 overflow-auto">
      <div className="space-y-6">
        <StackDetailView stackId={parseInt(id)} />
      </div>
    </div>
  )
}
