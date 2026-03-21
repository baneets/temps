import { StacksList } from '@/components/stacks/StacksList'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { usePageTitle } from '@/hooks/usePageTitle'
import { useEffect } from 'react'

export function Stacks() {
  const { setBreadcrumbs } = useBreadcrumbs()

  useEffect(() => {
    setBreadcrumbs([{ label: 'Stacks' }])
  }, [setBreadcrumbs])

  usePageTitle('Stacks')

  return (
    <div className="flex-1 overflow-auto">
      <div className="space-y-6">
        <StacksList />
      </div>
    </div>
  )
}
