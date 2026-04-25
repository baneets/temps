import { S3SourcesManagement } from '@/components/backups/S3SourcesManagement'
import { useBreadcrumbs } from '@/contexts/BreadcrumbContext'
import { usePageTitle } from '@/hooks/usePageTitle'
import { useEffect } from 'react'

export function Backups() {
  const { setBreadcrumbs } = useBreadcrumbs()

  useEffect(() => {
    setBreadcrumbs([{ label: 'Backups' }])
  }, [setBreadcrumbs])

  usePageTitle('Backups')

  return (
    <div className="flex-1 overflow-auto">
      <S3SourcesManagement />
    </div>
  )
}
