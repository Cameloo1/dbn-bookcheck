import "@fontsource-variable/manrope/wght.css";
import "@fontsource-variable/jetbrains-mono/wght.css";
import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { PortfolioExperience } from "../app/PortfolioExperience";
import type { PublicReport } from "../app/report-types";
import "../app/globals.css";

type ReportState =
  | { status: "loading" }
  | { status: "ready"; report: PublicReport }
  | { status: "error"; message: string };

function Application() {
  const [state, setState] = useState<ReportState>({ status: "loading" });

  useEffect(() => {
    const controller = new AbortController();
    const reportUrl = `${import.meta.env.BASE_URL}data/report.v1.json`;

    fetch(reportUrl, { signal: controller.signal })
      .then((response) => {
        if (!response.ok) {
          throw new Error(`report request failed with status ${response.status}`);
        }
        return response.json() as Promise<PublicReport>;
      })
      .then((report) => setState({ status: "ready", report }))
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        const message =
          error instanceof Error ? error.message : "unknown report loading error";
        setState({ status: "error", message });
      });

    return () => controller.abort();
  }, []);

  if (state.status === "loading") {
    return (
      <main className="boot-state" aria-live="polite">
        <p>Loading the reviewed DBN/ES evidence report…</p>
      </main>
    );
  }

  if (state.status === "error") {
    return (
      <main className="boot-state boot-state-error" role="alert">
        <h1>Evidence report unavailable</h1>
        <p>{state.message}</p>
        <p>No fallback metrics were invented.</p>
      </main>
    );
  }

  return <PortfolioExperience report={state.report} />;
}

const root = document.getElementById("root");
if (!root) throw new Error("missing #root application mount");

createRoot(root).render(
  <StrictMode>
    <Application />
  </StrictMode>,
);
