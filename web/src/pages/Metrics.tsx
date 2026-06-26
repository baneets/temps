import { Routes, Route } from 'react-router-dom'
import { ProjectResponse } from '@/api/client'
import MetricsExplorer from './MetricsExplorer'

interface MetricsProps {
  project: ProjectResponse
}

/**
 * Metrics section router. Mirrors `Traces.tsx`: an index explorer and room to
 * grow a `:metricName` detail drill-down later. Mounted at `metrics/*` under
 * the project (see ProjectDetail routes).
 */
export default function Metrics({ project }: MetricsProps) {
  return (
    <Routes>
      <Route index element={<MetricsExplorer project={project} />} />
    </Routes>
  )
}
