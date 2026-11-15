/**
 * Shared state surfaces, re-exported from the household design system.
 *
 * These were local copies until the canonical-shape alignment: the package
 * ships all three, and a local copy cannot receive an upstream fix. The barrel
 * keeps `#lib/components/common` as the import path callers already use, so the
 * swap is invisible at every call site.
 */
export { EmptyState } from '@poodle64/ui/empty-state';
export { ErrorState } from '@poodle64/ui/error-state';
export { LoadingState } from '@poodle64/ui/loading-state';
