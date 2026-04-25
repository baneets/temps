import { useTempsAnalytics } from "./useTempsAnalytics";

export function useTrackPageview(): () => void {
  const analytics = useTempsAnalytics();
  return () => analytics.trackPageview();
}
