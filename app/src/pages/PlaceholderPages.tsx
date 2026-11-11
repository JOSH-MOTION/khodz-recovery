import { EmptyState, PageHeader } from "../components/ui";

// These pages intentionally show no data: their engines are not implemented yet, and the product
// never displays simulated drives or results. Each says which stage delivers it.

export function ResultsPage() {
  return (
    <>
      <PageHeader title="Results" subtitle="Browse files found by a scan and preview them before recovering." />
      <EmptyState icon="results" title="No scan results" badge="Stages 5–6">
        Results from quick and deep scans will appear here once scanning is implemented.
      </EmptyState>
    </>
  );
}

export function RecoveryPage() {
  return (
    <>
      <PageHeader title="Recovery" subtitle="Copy selected files to a safe destination and verify them." />
      <EmptyState icon="recover" title="Nothing queued for recovery" badge="Stage 7">
        Recovered files are always written to a different physical disk than the source and verified with SHA-256
        after writing.
      </EmptyState>
    </>
  );
}
