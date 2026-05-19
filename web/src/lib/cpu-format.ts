// Docker reports container CPU as a multi-core percentage: 200% = 2 cores
// fully pinned. That number is meaningless to a user reading a stat strip,
// so we render it as decimal cores used (and `/ limit` when a cap is set).
//
// Examples:
//   formatCpuUsage(200, 2)    -> "2.00 / 2 cores"
//   formatCpuUsage(140, 2)    -> "1.40 / 2 cores"
//   formatCpuUsage(85, null)  -> "0.85 cores"
//   formatCpuUsage(2.4, 1)    -> "0.024 / 1 core"
//   formatCpuUsage(0, 2)      -> "0.00 / 2 cores"

export function coresFromPercent(percent: number | null | undefined): number | null {
  if (percent == null || !Number.isFinite(percent)) return null
  return percent / 100
}

// Always decimal cores. Pick precision so small values still show signal:
//   >= 1 core   -> 2 decimals  (e.g. "1.40")
//   >= 0.1      -> 2 decimals  (e.g. "0.85")
//   < 0.1       -> 3 decimals  (e.g. "0.024", "0.003")
function formatCoreValue(cores: number): string {
  const abs = Math.abs(cores)
  if (abs >= 0.1) return cores.toFixed(2)
  return cores.toFixed(3)
}

function pluralizeCores(limitCores: number): string {
  return limitCores === 1 ? 'core' : 'cores'
}

function formatLimitCores(limitCores: number): string {
  // Whole-number limits read as "1" / "2" — the common config. Fractional
  // limits (e.g. 0.5) render with enough precision to round-trip.
  if (Number.isInteger(limitCores)) return String(limitCores)
  return limitCores >= 0.1 ? limitCores.toFixed(2) : limitCores.toFixed(3)
}

export function formatCpuUsage(
  cpuPercent: number | null | undefined,
  limitCores: number | null | undefined,
): string {
  const cores = coresFromPercent(cpuPercent)
  if (cores == null) return '—'

  const value = formatCoreValue(cores)

  if (limitCores != null && limitCores > 0) {
    return `${value} / ${formatLimitCores(limitCores)} ${pluralizeCores(limitCores)}`
  }

  return `${value} cores`
}

// Percent of the configured CPU cap (or null if no cap). Used to drive
// progress bars that should fill at the cap, not at 100% of all host cores.
export function cpuPercentOfLimit(
  cpuPercent: number | null | undefined,
  limitCores: number | null | undefined,
): number | null {
  if (cpuPercent == null || !Number.isFinite(cpuPercent)) return null
  if (limitCores == null || limitCores <= 0) return null
  return cpuPercent / limitCores
}
