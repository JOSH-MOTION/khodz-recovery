import { Button, EmptyState, PageHeader } from "../components/ui";

// These pages intentionally show no data: their engines are not implemented yet, and the product
// never displays simulated drives or results. Each says which stage delivers it.

export function DrivesPage() {
  return (
    <>
      <PageHeader
        title="Drives"
        subtitle="Choose a drive or partition to scan for lost files."
        actions={
          <Button icon="refresh" disabled title="Drive detection is not available in this build">
            Refresh
          </Button>
        }
      />
      <EmptyState icon="drive" title="Drive detection isn't available yet" badge="Stage 2">
        Physical drive and volume discovery for Windows and macOS is the next milestone. Until then, no drives
        are listed — Khodz Recovery never shows placeholder devices.
      </EmptyState>
    </>
  );
}

export function ImagesPage() {
  return (
    <>
      <PageHeader
        title="Disk Images"
        subtitle="Scan a raw disk image (.img, .dd, .bin) instead of a live drive."
        actions={
          <Button variant="primary" icon="plus" disabled title="Image scanning is not available in this build">
            Open image
          </Button>
        }
      />
      <EmptyState icon="image" title="Image scanning isn't available yet" badge="Stage 3">
        Imaging a failing drive first and scanning the image is the safest way to recover data. Read-only image
        access arrives with the storage layer.
      </EmptyState>
    </>
  );
}

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
