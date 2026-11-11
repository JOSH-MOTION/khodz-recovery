import { useCallback, useState } from "react";
import { Sidebar, type Route } from "./components/Sidebar";
import { HistoryPage } from "./pages/HistoryPage";
import { DrivesPage } from "./pages/DrivesPage";
import { DiskImagesPage } from "./pages/DiskImagesPage";
import { RecoveryPage } from "./pages/PlaceholderPages";
import { ResultsPage } from "./pages/ResultsPage";
import { SettingsPage } from "./pages/SettingsPage";
import { ScanProvider } from "./lib/scan";
import { useTheme } from "./lib/theme";

export default function App() {
  const [route, setRoute] = useState<Route>("drives");
  const theme = useTheme();
  const showResults = useCallback(() => setRoute("results"), []);

  return (
    <ScanProvider onStarted={showResults}>
      <div className="flex h-full bg-bg text-fg">
        <Sidebar route={route} onNavigate={setRoute} />
        <main key={route} className="k-enter flex-1 overflow-y-auto">
          {route === "drives" && <DrivesPage />}
          {route === "images" && <DiskImagesPage />}
          {route === "results" && <ResultsPage />}
          {route === "recovery" && <RecoveryPage />}
          {route === "history" && <HistoryPage />}
          {route === "settings" && <SettingsPage theme={theme.preference} onThemeChange={theme.setPreference} />}
        </main>
      </div>
    </ScanProvider>
  );
}
