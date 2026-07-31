import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

const sectionIds = [
  "overview",
  "data",
  "pipeline",
  "decode",
  "book",
  "validation",
  "sweeps",
  "benchmarks",
  "parity",
  "limits",
];

async function openPortfolio(page: Page) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/", { waitUntil: "networkidle" });
  await expect(page.locator("#main-content")).toBeVisible();
}

test("renders the complete guided report with truthful provenance", async ({
  page,
}) => {
  await openPortfolio(page);

  await expect(
    page.getByRole("heading", {
      level: 1,
      name: /Reconstructing ES from 58,988,994 market events/u,
    }),
  ).toBeVisible();
  for (const label of ["Live measured", "Live derived", "Synthetic"]) {
    await expect(
      page
        .locator(".evidence-badge:visible")
        .filter({ hasText: label })
        .first(),
    ).toBeVisible();
  }
  await expect(page.getByText("Your site is taking shape")).toHaveCount(0);
  await expect(page.getByText("Building your site")).toHaveCount(0);

  const renderedIds = await page
    .locator("[data-story-step]")
    .evaluateAll((sections) => sections.map((section) => section.id));
  expect(renderedIds).toEqual(sectionIds);

  const rail = page.getByRole("complementary", {
    name: "Case study progress",
  });
  for (const id of sectionIds) {
    await expect(rail.locator(`a[href="#${id}"]`)).toHaveCount(1);
  }

  const evidenceButton = page
    .locator(".headline-metric")
    .first()
    .getByRole("button", { name: /Evidence/u });
  await evidenceButton.click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("How this was measured")).toBeVisible();
  await expect(dialog.getByText("Source", { exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "Close evidence details" }).click();
  await expect(dialog).toBeHidden();
});

test("serves the public report JSON without private or paid payload data", async ({
  page,
  request,
}) => {
  const response = await request.get("/data/report.v1.json");
  expect(response.ok()).toBeTruthy();
  expect(response.headers()["content-type"]).toMatch(/^application\/json\b/iu);

  const body = await response.text();
  const report = JSON.parse(body);
  expect(report.schema_version).toBe(1);
  expect(report.dataset.total_records).toBe(58_988_994);
  expect(report.validation.exact_matches).toBe(23_961_616);
  expect(report.sweep.event_count).toBe(11);
  expect(report.synthetic.book_replay.evidence_status).toBe("synthetic");

  await openPortfolio(page);
  // Development module identifiers may contain local paths, but those are not
  // part of the production bundle or the user-visible portfolio content.
  const exposedText = `${body}\n${await page.locator("body").innerText()}`;
  expect(exposedText).not.toMatch(
    /(?:\b[A-Za-z]:[\\/]|\\\\[A-Za-z0-9._-]+\\[A-Za-z0-9$._-]+(?:\\[^\s"'<>|]+)*|\/(?:Users|home)\/[^/\s]+)/u,
  );
  expect(exposedText).not.toMatch(/\.(?:dbn|dbn\.zst|parquet|feather)\b/iu);
  expect(exposedText).not.toMatch(
    /(?:DATABENTO_API_KEY|OPENAI_API_KEY|(?:db|sk)-(?:live-)?[A-Za-z0-9_-]{20,})/u,
  );
});

test("supports story navigation and the primary interactive controls", async ({
  page,
}) => {
  await openPortfolio(page);

  await page.getByRole("link", { name: "Explore all benchmarks" }).click();
  await expect(page).toHaveURL(/#benchmarks$/u);
  await page.locator("#benchmarks").scrollIntoViewIfNeeded();
  await expect(page.locator("#benchmarks")).toBeInViewport();

  const dataset = page.locator("#data");
  await dataset.getByRole("button", { name: "Cost", exact: true }).click();
  await expect(
    dataset.getByRole("button", { name: "Cost", exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await dataset.getByRole("button", { name: /^Trades\b/u }).click();
  await expect(dataset.getByRole("heading", { level: 3, name: "Trades" })).toBeVisible();
  await expect(dataset.locator(".schema-inspector")).toContainText("Quoted cost");

  const book = page.locator("#book");
  await expect(book).toContainText("Event 1 of 8");
  await book.getByRole("button", { name: "Next event" }).click();
  await expect(book).toContainText("Event 2 of 8");
  await expect(book.getByRole("heading", { level: 3, name: "Snapshot begins" })).toBeVisible();

  const sweeps = page.locator("#sweeps");
  const measuredEvents = sweeps.locator(".measured-sweep-card > strong");
  await expect(measuredEvents).toHaveText("11");
  const penetration = sweeps.locator('input[type="range"]').nth(1);
  await penetration.fill("8");
  await expect(penetration).toHaveValue("8");
  await expect(sweeps.getByText("8 ticks", { exact: true })).toBeVisible();
  await expect(measuredEvents).toHaveText("11");

  const benchmarks = page.locator("#benchmarks");
  await benchmarks.getByLabel("Schema").selectOption("mbo");
  await benchmarks.getByLabel("Compression").selectOption("none");
  await benchmarks.getByLabel("Access").selectOption("fully_buffered_input");
  await benchmarks
    .getByLabel("Concurrency")
    .selectOption("parallel_independent_streams");
  await expect(benchmarks.locator(".benchmark-summary")).toContainText(
    "Showing 1 measured rows",
  );
  await expect
    .poll(() => new URL(page.url()).searchParams.get("schema"))
    .toBe("mbo");

  const shell = page.locator(".site-shell");
  await expect(shell).toHaveClass(/theme-observatory/u);
  await page
    .getByRole("button", { name: "Switch to research ledger view" })
    .click();
  await expect(shell).toHaveClass(/theme-ledger/u);
  await expect(
    page.getByRole("button", {
      name: "Switch to exchange observatory view",
    }),
  ).toBeVisible();
});

test("has no serious accessibility violations on the full report", async ({
  page,
}) => {
  await openPortfolio(page);
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"])
    .analyze();
  const blockingViolations = results.violations.filter(
    ({ impact }) => impact === "critical" || impact === "serious",
  );

  expect(
    blockingViolations.map(({ id, impact, nodes }) => ({
      id,
      impact,
      affectedNodes: nodes.length,
      targets: nodes.flatMap((node) => node.target).slice(0, 8),
      failures: nodes
        .map((node) => node.failureSummary)
        .filter((summary): summary is string => Boolean(summary)),
    })),
  ).toEqual([]);
});

test("loads and interacts without browser console errors", async ({ page }) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  page.on("pageerror", (error) => pageErrors.push(error.message));

  await openPortfolio(page);
  await page.locator("#pipeline").getByRole("button", { name: "Recovery path" }).click();
  await page.locator("#book").getByRole("button", { name: "Next event" }).click();
  await page
    .getByRole("button", { name: "Switch to research ledger view" })
    .click();

  expect(consoleErrors, consoleErrors.join("\n")).toEqual([]);
  expect(pageErrors, pageErrors.join("\n")).toEqual([]);
});
