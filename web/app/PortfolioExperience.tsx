"use client";

import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import type {
  BenchmarkRow,
  DatasetSchema,
  PublicClaim,
  PublicReport,
} from "./report-types";

const STEPS = [
  { id: "overview", label: "Overview", eyebrow: "00" },
  { id: "data", label: "The data", eyebrow: "01" },
  { id: "pipeline", label: "Pipeline", eyebrow: "02" },
  { id: "decode", label: "Decode", eyebrow: "03" },
  { id: "book", label: "Book replay", eyebrow: "04" },
  { id: "validation", label: "Validation", eyebrow: "05" },
  { id: "sweeps", label: "Sweep lab", eyebrow: "06" },
  { id: "benchmarks", label: "Benchmarks", eyebrow: "07" },
  { id: "parity", label: "Engineering", eyebrow: "08" },
  { id: "limits", label: "Limits", eyebrow: "09" },
] as const;

type DatasetMetric = "records" | "compressed_bytes" | "decoded_bytes" | "cost_usd";
type BenchmarkMetric =
  | "messages_per_second_median"
  | "decoded_mib_per_second_median"
  | "nanoseconds_per_message_median"
  | "peak_rss_mib_median";

const DATASET_METRICS: Array<{
  key: DatasetMetric;
  label: string;
  unit: string;
}> = [
  { key: "records", label: "Records", unit: "records" },
  { key: "compressed_bytes", label: "Compressed", unit: "bytes" },
  { key: "decoded_bytes", label: "Decoded", unit: "bytes" },
  { key: "cost_usd", label: "Cost", unit: "USD" },
];

const BENCHMARK_METRICS: Array<{
  key: BenchmarkMetric;
  label: string;
  short: string;
}> = [
  {
    key: "messages_per_second_median",
    label: "Median messages / second",
    short: "msg/s",
  },
  {
    key: "decoded_mib_per_second_median",
    label: "Decoded MiB / second",
    short: "MiB/s",
  },
  {
    key: "nanoseconds_per_message_median",
    label: "Nanoseconds / message",
    short: "ns/msg",
  },
  {
    key: "peak_rss_mib_median",
    label: "Median peak memory",
    short: "MiB RSS",
  },
];

const PIPELINE_STAGES = [
  {
    id: "dbn",
    label: "DBN / Zstd",
    contract: "Checksum-verified, schema-matched files from one bounded session.",
    recovery:
      "Atomic delivery, spend ledger, and no automatic replay when delivery is uncertain.",
  },
  {
    id: "decoder",
    label: "Strict decoder",
    contract: "Typed, fallible streaming records with one-pass truncation checks.",
    recovery:
      "Malformed headers, invalid compression, and truncated records fail explicitly.",
  },
  {
    id: "book",
    label: "MBO book",
    contract:
      "Per-instrument order state; trustworthy only after a complete snapshot boundary.",
    recovery:
      "Bad-book flags or inconsistent transitions invalidate state until a new baseline.",
  },
  {
    id: "validation",
    label: "MBP-10 check",
    contract:
      "Strict merge on timestamp, publisher, instrument, sequence, action, price, and size.",
    recovery:
      "Unmatched eligible exchange updates remain failures in the denominator.",
  },
  {
    id: "analysis",
    label: "Sweep heuristic",
    contract:
      "Four explicit parameters, pre-event visible book state, monotonic event output.",
    recovery:
      "Expired candidates are discarded; invalid book state suspends analysis.",
  },
  {
    id: "interfaces",
    label: "Rust / Node",
    contract:
      "One core owns market logic; CLI and N-API binding expose bounded interfaces.",
    recovery:
      "64-bit timestamps, prices, sizes, and counts stay bigint-safe at the JS boundary.",
  },
];

interface BookEvent {
  action: string;
  label: string;
  detail: string;
  trustworthy: boolean;
  compare: "none" | "pre-event" | "final-event";
  bids: Array<[number, number]>;
  asks: Array<[number, number]>;
}

const BOOK_EVENTS: BookEvent[] = [
  {
    action: "WAIT",
    label: "Mid-session arrival",
    detail:
      "The request begins without a complete order-book baseline. Records may decode, but the book is deliberately withheld.",
    trustworthy: false,
    compare: "none",
    bids: [],
    asks: [],
  },
  {
    action: "CLEAR",
    label: "Snapshot begins",
    detail:
      "A clear starts the exchange snapshot. Intermediate state is buffered through the event boundary.",
    trustworthy: false,
    compare: "none",
    bids: [],
    asks: [],
  },
  {
    action: "ADD",
    label: "Snapshot levels arrive",
    detail:
      "Bid and ask orders accumulate, but no comparison is emitted before the final snapshot flag.",
    trustworthy: false,
    compare: "none",
    bids: [
      [5249.75, 18],
      [5249.5, 31],
    ],
    asks: [
      [5250, 14],
      [5250.25, 27],
    ],
  },
  {
    action: "F_LAST",
    label: "Baseline complete",
    detail:
      "The complete snapshot closes. The reconstructed book is now eligible for independent comparison.",
    trustworthy: true,
    compare: "final-event",
    bids: [
      [5249.75, 18],
      [5249.5, 31],
    ],
    asks: [
      [5250, 14],
      [5250.25, 27],
    ],
  },
  {
    action: "TRADE",
    label: "Trade uses pre-event state",
    detail:
      "A trade compares MBP-10 with the book immediately before its event. The visible ask is still 14 here.",
    trustworthy: true,
    compare: "pre-event",
    bids: [
      [5249.75, 18],
      [5249.5, 31],
    ],
    asks: [
      [5250, 14],
      [5250.25, 27],
    ],
  },
  {
    action: "CANCEL",
    label: "Mutation uses final state",
    detail:
      "A cancel is buffered through F_LAST, then compared against the final aggregate book.",
    trustworthy: true,
    compare: "final-event",
    bids: [
      [5249.75, 11],
      [5249.5, 31],
    ],
    asks: [
      [5250, 14],
      [5250.25, 27],
    ],
  },
  {
    action: "BAD_BOOK",
    label: "State invalidated",
    detail:
      "An exchange bad-book flag invalidates the reconstructed state. The pipeline does not guess through the gap.",
    trustworthy: false,
    compare: "none",
    bids: [],
    asks: [],
  },
  {
    action: "RECOVER",
    label: "New complete baseline",
    detail:
      "A later complete clear/snapshot restores trustworthy state and validation resumes.",
    trustworthy: true,
    compare: "final-event",
    bids: [
      [5250, 22],
      [5249.75, 36],
    ],
    asks: [
      [5250.25, 19],
      [5250.5, 41],
    ],
  },
];

const SYNTHETIC_TRADES = Array.from({ length: 96 }, (_, index) => {
  let price = 5250 + Math.round(Math.sin(index * 0.62)) * 0.25;
  if (index >= 55 && index <= 57) price = 5251.5 + (index - 55) * 0.5;
  if (index >= 58 && index <= 61) price = 5251.75 - (index - 58) * 0.5;
  if (index >= 77 && index <= 79) price = 5248.5 - (index - 77) * 0.5;
  if (index >= 80 && index <= 83) price = 5248 + (index - 80) * 0.75;
  return { index, timestampMs: index * 250, price };
});

const formatInteger = new Intl.NumberFormat("en-US", {
  maximumFractionDigits: 0,
});
const formatDecimal = new Intl.NumberFormat("en-US", {
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});
const formatMoney = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 6,
});

function formatBytes(bytes: number): string {
  if (bytes < 1_000) return `${bytes} B`;
  if (bytes < 1_000_000) return `${formatDecimal.format(bytes / 1_000)} KB`;
  if (bytes < 1_000_000_000) return `${formatDecimal.format(bytes / 1_000_000)} MB`;
  return `${formatDecimal.format(bytes / 1_000_000_000)} GB`;
}

function formatDatasetValue(schema: DatasetSchema, metric: DatasetMetric): string {
  if (metric === "records") return formatInteger.format(schema.records);
  if (metric === "cost_usd") return formatMoney.format(schema.cost_usd);
  return formatBytes(schema[metric]);
}

function formatBenchmarkValue(row: BenchmarkRow, metric: BenchmarkMetric): string {
  const value = row[metric];
  if (metric === "messages_per_second_median") {
    return formatInteger.format(value);
  }
  return formatDecimal.format(value);
}

function humanize(value: string): string {
  return value
    .replaceAll("_", " ")
    .replace(/\b\w/gu, (character) => character.toUpperCase());
}

function statusLabel(status: PublicClaim["evidence_status"]): string {
  if (status === "measured-live") return "Live measured";
  if (status === "derived-live") return "Live derived";
  if (status === "synthetic") return "Synthetic";
  return "Unsupported";
}

function GlyphField() {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const context = canvas.getContext("2d");
    if (!context) return;

    const reducedMotion = window.matchMedia(
      "(prefers-reduced-motion: reduce)",
    ).matches;
    let frame = 0;
    let animation = 0;
    let visible = true;
    let pointerX = 0.7;
    let pointerY = 0.35;

    const resize = () => {
      const ratio = Math.min(window.devicePixelRatio || 1, 2);
      const bounds = canvas.getBoundingClientRect();
      canvas.width = Math.max(1, Math.round(bounds.width * ratio));
      canvas.height = Math.max(1, Math.round(bounds.height * ratio));
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
    };

    const draw = () => {
      const bounds = canvas.getBoundingClientRect();
      context.clearRect(0, 0, bounds.width, bounds.height);
      context.font = "11px 'JetBrains Mono Variable', monospace";
      context.textAlign = "center";
      context.textBaseline = "middle";

      const columns = Math.ceil(bounds.width / 24);
      const rows = Math.ceil(bounds.height / 24);
      const glyphs = ["·", "+", "×", ":", "□", "∆"];
      for (let row = 0; row < rows; row += 1) {
        for (let column = 0; column < columns; column += 1) {
          const x = column * 24 + 12;
          const y = row * 24 + 12;
          const dx = x / Math.max(bounds.width, 1) - pointerX;
          const dy = y / Math.max(bounds.height, 1) - pointerY;
          const distance = Math.sqrt(dx * dx + dy * dy);
          const wave = Math.sin(column * 0.8 + row * 0.55 + frame * 0.02);
          const alpha = Math.max(0.05, 0.28 - distance * 0.24 + wave * 0.035);
          context.fillStyle = `rgba(132, 241, 211, ${alpha})`;
          context.fillText(glyphs[(column + row * 2) % glyphs.length], x, y);
        }
      }
      frame += 1;
      if (!reducedMotion && visible && !document.hidden) {
        animation = window.requestAnimationFrame(draw);
      }
    };

    const onPointer = (event: PointerEvent) => {
      const bounds = canvas.getBoundingClientRect();
      pointerX = (event.clientX - bounds.left) / Math.max(bounds.width, 1);
      pointerY = (event.clientY - bounds.top) / Math.max(bounds.height, 1);
    };
    const onVisibility = () => {
      window.cancelAnimationFrame(animation);
      if (!document.hidden && visible) draw();
    };
    const observer = new IntersectionObserver(([entry]) => {
      visible = entry.isIntersecting;
      window.cancelAnimationFrame(animation);
      if (visible) draw();
    });
    const resizeObserver = new ResizeObserver(() => {
      resize();
      draw();
    });

    resize();
    observer.observe(canvas);
    resizeObserver.observe(canvas);
    canvas.addEventListener("pointermove", onPointer);
    document.addEventListener("visibilitychange", onVisibility);
    draw();

    return () => {
      window.cancelAnimationFrame(animation);
      observer.disconnect();
      resizeObserver.disconnect();
      canvas.removeEventListener("pointermove", onPointer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);

  return <canvas ref={canvasRef} className="glyph-field" aria-hidden="true" />;
}

function EvidenceBadge({ status }: { status: PublicClaim["evidence_status"] }) {
  return (
    <span className={`evidence-badge evidence-${status}`}>
      <span aria-hidden="true" className="badge-dot" />
      {statusLabel(status)}
    </span>
  );
}

function SectionHeader({
  step,
  kicker,
  title,
  children,
}: {
  step: string;
  kicker: string;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <header className="section-header">
      <div className="section-index" aria-hidden="true">
        {step}
      </div>
      <div>
        <p className="eyebrow">{kicker}</p>
        <h2>{title}</h2>
        <div className="section-intro">{children}</div>
      </div>
    </header>
  );
}

function SourceDialog({
  claim,
  onClose,
}: {
  claim: PublicClaim | null;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (claim && dialog && !dialog.open) dialog.showModal();
    if (!claim && dialog?.open) dialog.close();
  }, [claim]);

  return (
    <dialog
      ref={dialogRef}
      className="source-dialog"
      onClose={onClose}
      onCancel={onClose}
      aria-labelledby="source-dialog-title"
    >
      {claim ? (
        <div className="source-dialog-inner">
          <div className="source-dialog-topline">
            <EvidenceBadge status={claim.evidence_status} />
            <button type="button" className="icon-button" onClick={onClose}>
              <span aria-hidden="true">×</span>
              <span className="sr-only">Close evidence details</span>
            </button>
          </div>
          <p className="eyebrow">How this was measured</p>
          <h2 id="source-dialog-title">{claim.label}</h2>
          <p className="source-value">{claim.display}</p>
          <dl className="source-details">
            <div>
              <dt>Source</dt>
              <dd>
                <code>{claim.source_path}</code>
                <span>{claim.source_locator}</span>
              </dd>
            </div>
            <div>
              <dt>Method</dt>
              <dd>{claim.method_note}</dd>
            </div>
            <div>
              <dt>Boundary</dt>
              <dd>{claim.limitations}</dd>
            </div>
          </dl>
        </div>
      ) : null}
    </dialog>
  );
}

function detectSyntheticSweeps(
  lookback: number,
  thresholdTicks: number,
  windowMs: number,
) {
  let above = 0;
  let below = 0;
  const triggers = new Set<number>();

  for (let index = lookback; index < SYNTHETIC_TRADES.length; index += 1) {
    const current = SYNTHETIC_TRADES[index];
    const prior = SYNTHETIC_TRADES.slice(index - lookback, index);
    const priorHigh = Math.max(...prior.map((trade) => trade.price));
    const priorLow = Math.min(...prior.map((trade) => trade.price));
    const threshold = thresholdTicks * 0.25;
    const deadline = current.timestampMs + windowMs;

    if (current.price >= priorHigh + threshold) {
      const reverted = SYNTHETIC_TRADES.slice(index + 1).find(
        (trade) => trade.timestampMs <= deadline && trade.price <= priorHigh,
      );
      if (reverted) {
        above += 1;
        triggers.add(index);
        index = reverted.index;
      }
    } else if (current.price <= priorLow - threshold) {
      const reverted = SYNTHETIC_TRADES.slice(index + 1).find(
        (trade) => trade.timestampMs <= deadline && trade.price >= priorLow,
      );
      if (reverted) {
        below += 1;
        triggers.add(index);
        index = reverted.index;
      }
    }
  }
  return { above, below, total: above + below, triggers };
}

export function PortfolioExperience({ report }: { report: PublicReport }) {
  const [activeStep, setActiveStep] = useState(0);
  const [theme, setTheme] = useState<"observatory" | "ledger">("observatory");
  const [sourceClaim, setSourceClaim] = useState<PublicClaim | null>(null);
  const [datasetMetric, setDatasetMetric] = useState<DatasetMetric>("records");
  const [selectedSchema, setSelectedSchema] = useState(
    report.dataset.schemas[0]?.name ?? "mbo",
  );
  const [pipelineStage, setPipelineStage] = useState("book");
  const [pipelineMode, setPipelineMode] = useState<"normal" | "recovery">(
    "normal",
  );
  const [bookIndex, setBookIndex] = useState(0);
  const [bookPlaying, setBookPlaying] = useState(false);
  const [bookSpeed, setBookSpeed] = useState(1);
  const [sweepLookback, setSweepLookback] = useState(
    report.sweep.parameters.lookback_trades,
  );
  const [sweepThreshold, setSweepThreshold] = useState(
    report.sweep.parameters.threshold_ticks,
  );
  const [sweepWindow, setSweepWindow] = useState(
    report.sweep.parameters.reversion_window_ms,
  );
  const [benchmarkSchema, setBenchmarkSchema] = useState("all");
  const [benchmarkCompression, setBenchmarkCompression] = useState("zstd");
  const [benchmarkAccess, setBenchmarkAccess] = useState("streaming");
  const [benchmarkConcurrency, setBenchmarkConcurrency] =
    useState("single_thread");
  const [benchmarkMetric, setBenchmarkMetric] =
    useState<BenchmarkMetric>("decoded_mib_per_second_median");
  const [copyMessage, setCopyMessage] = useState("");

  const claim = (needle: string) =>
    report.claims.find(
      (item) =>
        item.id === needle ||
        item.id.includes(needle) ||
        item.label.toLowerCase().includes(needle.replaceAll("_", " ")),
    ) ?? null;

  const headlineClaims = [
    claim("exact"),
    claim("records"),
    claim("throughput"),
    claim("cost"),
  ].filter((item): item is PublicClaim => item !== null);

  useEffect(() => {
    const sections = Array.from(
      document.querySelectorAll<HTMLElement>("[data-story-step]"),
    );
    const observer = new IntersectionObserver(
      (entries) => {
        const visible = entries
          .filter((entry) => entry.isIntersecting)
          .sort((a, b) => b.intersectionRatio - a.intersectionRatio)[0];
        if (!visible) return;
        const index = STEPS.findIndex(
          (step) => step.id === (visible.target as HTMLElement).id,
        );
        if (index >= 0) setActiveStep(index);
      },
      { rootMargin: "-28% 0px -58% 0px", threshold: [0, 0.2, 0.55] },
    );
    sections.forEach((section) => observer.observe(section));
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const parameters = new URLSearchParams(window.location.search);
    parameters.set("schema", benchmarkSchema);
    parameters.set("compression", benchmarkCompression);
    parameters.set("access", benchmarkAccess);
    parameters.set("concurrency", benchmarkConcurrency);
    parameters.set("metric", benchmarkMetric);
    const hash = STEPS[activeStep]?.id ?? "overview";
    window.history.replaceState(
      null,
      "",
      `${window.location.pathname}?${parameters.toString()}#${hash}`,
    );
  }, [
    activeStep,
    benchmarkAccess,
    benchmarkCompression,
    benchmarkConcurrency,
    benchmarkMetric,
    benchmarkSchema,
  ]);

  useEffect(() => {
    const restore = () => {
      const parameters = new URLSearchParams(window.location.search);
      setBenchmarkSchema(parameters.get("schema") ?? "all");
      setBenchmarkCompression(parameters.get("compression") ?? "zstd");
      setBenchmarkAccess(parameters.get("access") ?? "streaming");
      setBenchmarkConcurrency(
        parameters.get("concurrency") ?? "single_thread",
      );
      const metric = parameters.get("metric") as BenchmarkMetric | null;
      if (BENCHMARK_METRICS.some((item) => item.key === metric)) {
        setBenchmarkMetric(metric as BenchmarkMetric);
      }
    };
    restore();
    window.addEventListener("popstate", restore);
    return () => window.removeEventListener("popstate", restore);
  }, []);

  useEffect(() => {
    if (!bookPlaying) return;
    const timer = window.setInterval(() => {
      setBookIndex((current) => {
        if (current >= BOOK_EVENTS.length - 1) {
          setBookPlaying(false);
          return current;
        }
        return current + 1;
      });
    }, 1500 / bookSpeed);
    return () => window.clearInterval(timer);
  }, [bookPlaying, bookSpeed]);

  const selectedDataset =
    report.dataset.schemas.find((schema) => schema.name === selectedSchema) ??
    report.dataset.schemas[0];
  const datasetMax = Math.max(
    ...report.dataset.schemas.map((schema) => schema[datasetMetric]),
    1,
  );
  const activePipeline =
    PIPELINE_STAGES.find((stage) => stage.id === pipelineStage) ??
    PIPELINE_STAGES[0];
  const bookEvent = BOOK_EVENTS[bookIndex];
  const sweepResult = useMemo(
    () => detectSyntheticSweeps(sweepLookback, sweepThreshold, sweepWindow),
    [sweepLookback, sweepThreshold, sweepWindow],
  );
  const filteredBenchmarks = useMemo(
    () =>
      report.benchmark.results.filter(
        (row) =>
          (benchmarkSchema === "all" || row.schema === benchmarkSchema) &&
          (benchmarkCompression === "all" ||
            row.compression === benchmarkCompression) &&
          (benchmarkAccess === "all" || row.access === benchmarkAccess) &&
          (benchmarkConcurrency === "all" ||
            row.concurrency === benchmarkConcurrency),
      ),
    [
      benchmarkAccess,
      benchmarkCompression,
      benchmarkConcurrency,
      benchmarkSchema,
      report.benchmark.results,
    ],
  );
  const benchmarkMax = Math.max(
    ...filteredBenchmarks.map((row) => row[benchmarkMetric]),
    1,
  );
  const maxMemory = Math.max(
    ...filteredBenchmarks.map((row) => row.peak_rss_mib_median),
    1,
  );
  const maxThroughput = Math.max(
    ...filteredBenchmarks.map((row) => row.messages_per_second_median),
    1,
  );

  const copyText = async (text: string, label: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopyMessage(`${label} copied`);
    } catch {
      setCopyMessage(`Copy unavailable. Select the ${label.toLowerCase()} manually.`);
    }
    window.setTimeout(() => setCopyMessage(""), 1800);
  };

  return (
    <div className={`site-shell theme-${theme}`}>
      <a className="skip-link" href="#main-content">
        Skip to case study
      </a>
      <div className="topbar">
        <a className="wordmark" href="#overview" aria-label="DBN ES case study home">
          <span aria-hidden="true">DBN/ES</span>
          <span className="wordmark-full">Market systems case study</span>
        </a>
        <div className="topbar-actions">
          <EvidenceBadge status="measured-live" />
          <button
            type="button"
            className="theme-toggle"
            onClick={() =>
              setTheme((current) =>
                current === "observatory" ? "ledger" : "observatory",
              )
            }
            aria-label={`Switch to ${
              theme === "observatory" ? "research ledger" : "exchange observatory"
            } view`}
          >
            <span aria-hidden="true">{theme === "observatory" ? "◐" : "◑"}</span>
            {theme === "observatory" ? "Ledger view" : "Observatory view"}
          </button>
        </div>
      </div>

      <aside className="story-rail" aria-label="Case study progress">
        <div className="rail-track" aria-hidden="true">
          <span
            style={
              {
                "--progress": `${(activeStep / (STEPS.length - 1)) * 100}%`,
              } as CSSProperties
            }
          />
        </div>
        <ol>
          {STEPS.map((step, index) => (
            <li key={step.id}>
              <a
                href={`#${step.id}`}
                className={index === activeStep ? "is-active" : ""}
                aria-current={index === activeStep ? "step" : undefined}
              >
                <span>{step.eyebrow}</span>
                <strong>{step.label}</strong>
              </a>
            </li>
          ))}
        </ol>
      </aside>

      <main id="main-content">
        <section id="overview" className="hero" data-story-step>
          <GlyphField />
          <div className="hero-grid" aria-hidden="true" />
          <div className="hero-content">
            <div className="hero-kicker">
              <EvidenceBadge status="measured-live" />
              <span>
                {report.dataset.name} · {report.dataset.symbol} ·{" "}
                {report.dataset.session_hours}-hour session
              </span>
            </div>
            <h1>
              Reconstructing ES from{" "}
              <span>{formatInteger.format(report.dataset.total_records)}</span>{" "}
              market events.
            </h1>
            <p className="hero-lede">
              A Rust and Node market-data engineering case study: bounded
              acquisition, streaming decode, order-book reconstruction,
              independent validation, and measured performance.
            </p>
            <div className="hero-actions">
              <a className="button button-primary" href="#data">
                Take the five-minute tour
                <span aria-hidden="true">↓</span>
              </a>
              <a className="button button-secondary" href="#benchmarks">
                Explore all benchmarks
              </a>
            </div>
          </div>

          <div className="headline-metrics" aria-label="Headline project results">
            {headlineClaims.map((item) => (
              <article className="headline-metric" key={item.id}>
                <div>
                  <EvidenceBadge status={item.evidence_status} />
                  <strong>{item.display}</strong>
                  <span>{item.label}</span>
                </div>
                <button
                  type="button"
                  className="source-link"
                  onClick={() => setSourceClaim(item)}
                >
                  Evidence <span aria-hidden="true">↗</span>
                </button>
              </article>
            ))}
          </div>
          <p className="hero-boundary">
            One historical session. Warm page cache. No trading, P&amp;L, or
            signal-quality claim.
          </p>
        </section>

        <section id="data" className="story-section" data-story-step>
          <SectionHeader
            step="01"
            kicker="Bounded acquisition"
            title="Four views of the same market session."
          >
            <p>
              The acquisition was quoted before any external call, capped at
              $10, downloaded atomically, and checksum-verified. Each schema had
              one specific job.
            </p>
          </SectionHeader>

          <div className="control-strip" aria-label="Dataset comparison metric">
            {DATASET_METRICS.map((metric) => (
              <button
                key={metric.key}
                type="button"
                className={datasetMetric === metric.key ? "is-selected" : ""}
                aria-pressed={datasetMetric === metric.key}
                onClick={() => setDatasetMetric(metric.key)}
              >
                {metric.label}
              </button>
            ))}
          </div>

          <div className="dataset-layout">
            <div className="dataset-bars">
              {report.dataset.schemas.map((schema) => {
                const width = Math.max(
                  1.5,
                  (schema[datasetMetric] / datasetMax) * 100,
                );
                return (
                  <button
                    key={schema.name}
                    type="button"
                    className={`dataset-row ${
                      selectedSchema === schema.name ? "is-selected" : ""
                    }`}
                    onClick={() => setSelectedSchema(schema.name)}
                    aria-pressed={selectedSchema === schema.name}
                  >
                    <span className="dataset-row-label">
                      <strong>{schema.label}</strong>
                      <small>{formatDatasetValue(schema, datasetMetric)}</small>
                    </span>
                    <span className="dataset-bar-track" aria-hidden="true">
                      <span style={{ width: `${width}%` }} />
                    </span>
                  </button>
                );
              })}
            </div>

            {selectedDataset ? (
              <article className="schema-inspector" aria-live="polite">
                <div className="inspector-topline">
                  <span className="mono-chip">{selectedDataset.name}</span>
                  <EvidenceBadge status="measured-live" />
                </div>
                <h3>{selectedDataset.label}</h3>
                <p>{selectedDataset.used_for}</p>
                <dl className="mini-stat-grid">
                  <div>
                    <dt>Records</dt>
                    <dd>{formatInteger.format(selectedDataset.records)}</dd>
                  </div>
                  <div>
                    <dt>Compressed</dt>
                    <dd>{formatBytes(selectedDataset.compressed_bytes)}</dd>
                  </div>
                  <div>
                    <dt>Decoded</dt>
                    <dd>{formatBytes(selectedDataset.decoded_bytes)}</dd>
                  </div>
                  <div>
                    <dt>Quoted cost</dt>
                    <dd>{formatMoney.format(selectedDataset.cost_usd)}</dd>
                  </div>
                </dl>
              </article>
            ) : null}
          </div>

          <div className="total-ribbon">
            <span>Total acquired</span>
            <strong>{formatInteger.format(report.dataset.total_records)} records</strong>
            <strong>{formatBytes(report.dataset.compressed_bytes)} compressed</strong>
            <strong>{formatBytes(report.dataset.decoded_bytes)} decoded</strong>
            <strong>{formatMoney.format(report.dataset.total_cost_usd)}</strong>
          </div>
        </section>

        <section id="pipeline" className="story-section" data-story-step>
          <SectionHeader
            step="02"
            kicker="System boundary"
            title="Follow one record through the pipeline."
          >
            <p>
              The core owns decoding and market state. The CLI owns files,
              acquisition gates, reports, and orchestration. The Node binding
              exposes the same logic without reimplementing it.
            </p>
          </SectionHeader>

          <div className="segmented-control" aria-label="Pipeline path">
            <button
              type="button"
              aria-pressed={pipelineMode === "normal"}
              className={pipelineMode === "normal" ? "is-selected" : ""}
              onClick={() => setPipelineMode("normal")}
            >
              Normal path
            </button>
            <button
              type="button"
              aria-pressed={pipelineMode === "recovery"}
              className={pipelineMode === "recovery" ? "is-selected" : ""}
              onClick={() => setPipelineMode("recovery")}
            >
              Recovery path
            </button>
          </div>

          <div className="pipeline-flow" role="list" aria-label="Processing stages">
            {PIPELINE_STAGES.map((stage, index) => (
              <div
                className="pipeline-stage-wrap"
                key={stage.id}
                role="listitem"
              >
                <button
                  type="button"
                  className={`pipeline-stage ${
                    pipelineStage === stage.id ? "is-selected" : ""
                  }`}
                  aria-pressed={pipelineStage === stage.id}
                  onClick={() => setPipelineStage(stage.id)}
                >
                  <span>{String(index + 1).padStart(2, "0")}</span>
                  <strong>{stage.label}</strong>
                </button>
                {index < PIPELINE_STAGES.length - 1 ? (
                  <span className="pipeline-arrow" aria-hidden="true">
                    →
                  </span>
                ) : null}
              </div>
            ))}
          </div>

          <article className="pipeline-inspector" aria-live="polite">
            <div>
              <p className="eyebrow">
                {pipelineMode === "normal" ? "Contract" : "Failure and recovery"}
              </p>
              <h3>{activePipeline.label}</h3>
            </div>
            <p>
              {pipelineMode === "normal"
                ? activePipeline.contract
                : activePipeline.recovery}
            </p>
          </article>
        </section>

        <section id="decode" className="story-section" data-story-step>
          <SectionHeader
            step="03"
            kicker="Streaming decode"
            title="Bound memory first. Measure the tradeoff."
          >
            <p>
              The corpus expands to {formatBytes(report.dataset.decoded_bytes)}.
              The headline path streams compressed DBN with roughly 5.63 MiB
              peak RSS; buffered modes are faster in several rows but pay for
              that speed in memory.
            </p>
          </SectionHeader>

          <div className="decode-comparison">
            {report.benchmark.results
              .filter(
                (row) =>
                  row.schema === "mbo" &&
                  row.compression === "zstd" &&
                  row.concurrency === "single_thread",
              )
              .map((row) => (
                <article className="decode-card" key={row.access}>
                  <div className="decode-visual" aria-hidden="true">
                    <span className={`decode-blocks mode-${row.access}`}>
                      {Array.from({
                        length: row.access === "streaming" ? 6 : 18,
                      }).map((_, index) => (
                        <i key={index} />
                      ))}
                    </span>
                  </div>
                  <p className="eyebrow">{humanize(row.access)}</p>
                  <h3>
                    {formatInteger.format(row.messages_per_second_median)}{" "}
                    <span>msg/s</span>
                  </h3>
                  <dl>
                    <div>
                      <dt>Decoded rate</dt>
                      <dd>
                        {formatDecimal.format(
                          row.decoded_mib_per_second_median,
                        )}{" "}
                        MiB/s
                      </dd>
                    </div>
                    <div>
                      <dt>Peak RSS</dt>
                      <dd>{formatDecimal.format(row.peak_rss_mib_median)} MiB</dd>
                    </div>
                  </dl>
                </article>
              ))}
          </div>

          <aside className="method-note">
            <span className="method-glyph" aria-hidden="true">
              i
            </span>
            <p>
              <strong>Measured boundary:</strong> fresh child processes, one
              discarded warmup, five measured runs, warm page cache. Rates
              include file reads, decompression, parsing, traversal, thread
              startup, and joins.
            </p>
          </aside>
        </section>

        <section id="book" className="story-section" data-story-step>
          <SectionHeader
            step="04"
            kicker="Synthetic teaching replay"
            title="A book is only trustworthy after a baseline."
          >
            <p>
              Step through the correctness boundary that made the live
              validation meaningful. This sequence is deterministic and
              synthetic; it does not reproduce purchased market events.
            </p>
          </SectionHeader>

          <div className="replay-shell">
            <div className="replay-topbar">
              <EvidenceBadge status="synthetic" />
              <span>
                Event {bookIndex + 1} of {BOOK_EVENTS.length}
              </span>
              <span
                className={`trust-state ${
                  bookEvent.trustworthy ? "is-valid" : "is-withheld"
                }`}
              >
                {bookEvent.trustworthy ? "Book valid" : "Book withheld"}
              </span>
            </div>

            <div className="replay-main">
              <article className="event-tape" aria-live="polite">
                <span className="event-action">{bookEvent.action}</span>
                <p className="eyebrow">
                  {bookEvent.compare === "none"
                    ? "No comparison"
                    : `${bookEvent.compare} comparison`}
                </p>
                <h3>{bookEvent.label}</h3>
                <p>{bookEvent.detail}</p>
              </article>

              <div
                className={`book-ladder ${
                  bookEvent.trustworthy ? "" : "is-withheld"
                }`}
                aria-label={
                  bookEvent.trustworthy
                    ? "Synthetic two-level order book"
                    : "Order book withheld until a complete baseline"
                }
              >
                <div className="ladder-head">
                  <span>Bid size</span>
                  <span>Price</span>
                  <span>Ask size</span>
                </div>
                {bookEvent.trustworthy ? (
                  Array.from({ length: 4 }).map((_, index) => {
                    const ask = bookEvent.asks[bookEvent.asks.length - 1 - index];
                    const bid = bookEvent.bids[index - 2];
                    const row = ask ?? bid;
                    return (
                      <div
                        className={`ladder-row ${ask ? "ask-row" : "bid-row"}`}
                        key={`${index}-${row?.[0] ?? "empty"}`}
                      >
                        <span>{bid ? bid[1] : ""}</span>
                        <strong>{row ? row[0].toFixed(2) : "—"}</strong>
                        <span>{ask ? ask[1] : ""}</span>
                      </div>
                    );
                  })
                ) : (
                  <div className="withheld-message">
                    <span aria-hidden="true">∅</span>
                    Awaiting a complete snapshot boundary
                  </div>
                )}
              </div>
            </div>

            <div className="replay-controls">
              <button
                type="button"
                className="icon-button"
                disabled={bookIndex === 0}
                onClick={() => setBookIndex((index) => Math.max(0, index - 1))}
              >
                <span aria-hidden="true">←</span>
                <span className="sr-only">Previous event</span>
              </button>
              <button
                type="button"
                className="replay-play"
                onClick={() => {
                  if (bookIndex === BOOK_EVENTS.length - 1) setBookIndex(0);
                  setBookPlaying((playing) => !playing);
                }}
              >
                <span aria-hidden="true">{bookPlaying ? "Ⅱ" : "▶"}</span>
                {bookPlaying ? "Pause" : "Play"}
              </button>
              <button
                type="button"
                className="icon-button"
                disabled={bookIndex === BOOK_EVENTS.length - 1}
                onClick={() =>
                  setBookIndex((index) =>
                    Math.min(BOOK_EVENTS.length - 1, index + 1),
                  )
                }
              >
                <span aria-hidden="true">→</span>
                <span className="sr-only">Next event</span>
              </button>
              <label>
                Speed
                <select
                  value={bookSpeed}
                  onChange={(event) => setBookSpeed(Number(event.target.value))}
                >
                  <option value={0.75}>0.75×</option>
                  <option value={1}>1×</option>
                  <option value={1.5}>1.5×</option>
                  <option value={2}>2×</option>
                </select>
              </label>
              <div className="replay-dots" aria-label="Choose replay event">
                {BOOK_EVENTS.map((event, index) => (
                  <button
                    type="button"
                    key={`${event.action}-${index}`}
                    className={index === bookIndex ? "is-selected" : ""}
                    onClick={() => setBookIndex(index)}
                    aria-label={`Event ${index + 1}: ${event.label}`}
                    aria-current={index === bookIndex ? "step" : undefined}
                  />
                ))}
              </div>
            </div>
          </div>
        </section>

        <section id="validation" className="story-section" data-story-step>
          <SectionHeader
            step="05"
            kicker="Independent exchange view"
            title="Every eligible update stayed in the denominator."
          >
            <p>
              MBO reconstruction was compared with exchange-produced MBP-10 on
              a strict identity key. Pre-baseline updates are disclosed;
              eligible misses are never dropped.
            </p>
          </SectionHeader>

          <div className="validation-hero">
            <div>
              <EvidenceBadge status="measured-live" />
              <strong>
                {formatInteger.format(report.validation.exact_matches)}
                <span aria-hidden="true"> / </span>
                <span className="sr-only"> out of </span>
                {formatInteger.format(report.validation.aligned_updates)}
              </strong>
              <p>eligible updates matched price and aggregate size exactly</p>
            </div>
            <span className="validation-ring" aria-label="100 percent exact">
              100<small>%</small>
            </span>
          </div>

          <div className="waterfall" aria-label="Validation denominator">
            <article style={{ "--waterfall": "100%" } as CSSProperties}>
              <span>MBP-10 scanned</span>
              <strong>
                {formatInteger.format(report.validation.mbp10_records_scanned)}
              </strong>
              <i aria-hidden="true" />
            </article>
            <article
              style={
                {
                  "--waterfall": `${Math.max(
                    1,
                    (report.validation.mbp10_updates_before_valid_baseline /
                      report.validation.mbp10_records_scanned) *
                      100,
                  )}%`,
                } as CSSProperties
              }
            >
              <span>Before valid baseline</span>
              <strong>
                {formatInteger.format(
                  report.validation.mbp10_updates_before_valid_baseline,
                )}
              </strong>
              <i aria-hidden="true" />
            </article>
            <article
              style={
                {
                  "--waterfall": `${(report.validation.aligned_updates /
                    report.validation.mbp10_records_scanned) *
                    100}%`,
                } as CSSProperties
              }
            >
              <span>Eligible and aligned</span>
              <strong>
                {formatInteger.format(report.validation.aligned_updates)}
              </strong>
              <i aria-hidden="true" />
            </article>
            <article
              className="is-exact"
              style={
                {
                  "--waterfall": `${(report.validation.exact_matches /
                    report.validation.mbp10_records_scanned) *
                    100}%`,
                } as CSSProperties
              }
            >
              <span>Exact price + size</span>
              <strong>
                {formatInteger.format(report.validation.exact_matches)}
              </strong>
              <i aria-hidden="true" />
            </article>
          </div>

          <div className="merge-key">
            <p className="eyebrow">Strict alignment key</p>
            <div>
              {[
                "timestamp",
                "publisher",
                "instrument",
                "sequence",
                "action",
                "price",
                "size",
              ].map((key) => (
                <span key={key}>{key}</span>
              ))}
            </div>
          </div>

          <aside className="method-note">
            <span className="method-glyph" aria-hidden="true">
              ↔
            </span>
            <p>
              <strong>Why unmatched MBO observations are disclosed:</strong>{" "}
              multiple order events can exist without a paired MBP-10 record.
              The failure denominator is the other direction: every eligible
              MBP-10 update had to align, and zero went unmatched.
            </p>
          </aside>
        </section>

        <section id="sweeps" className="story-section" data-story-step>
          <SectionHeader
            step="06"
            kicker="Transparent heuristic"
            title="Keep the measured result locked. Experiment safely."
          >
            <p>
              The detector asks one narrow question: did price penetrate a
              recent trade extreme by a fixed number of ticks, then cross back
              through that level within a fixed window?
            </p>
          </SectionHeader>

          <div className="sweep-result-grid">
            <article className="measured-sweep-card">
              <EvidenceBadge status="measured-live" />
              <p className="eyebrow">Committed parameters</p>
              <strong>{report.sweep.event_count}</strong>
              <h3>heuristic events</h3>
              <div className="direction-split">
                <span>
                  <i className="up" aria-hidden="true" />
                  {report.sweep.above_high_count} above highs
                </span>
                <span>
                  <i className="down" aria-hidden="true" />
                  {report.sweep.below_low_count} below lows
                </span>
              </div>
              <dl>
                <div>
                  <dt>Lookback</dt>
                  <dd>{report.sweep.parameters.lookback_trades} trades</dd>
                </div>
                <div>
                  <dt>Threshold</dt>
                  <dd>{report.sweep.parameters.threshold_ticks} ticks</dd>
                </div>
                <div>
                  <dt>Reversion</dt>
                  <dd>
                    {formatInteger.format(
                      report.sweep.parameters.reversion_window_ms,
                    )}{" "}
                    ms
                  </dd>
                </div>
              </dl>
              <p className="locked-note">
                <span aria-hidden="true">▣</span> Controls do not change this
                measured card.
              </p>
            </article>

            <article className="sweep-lab">
              <div className="sweep-lab-head">
                <div>
                  <EvidenceBadge status="synthetic" />
                  <h3>Parameter lab</h3>
                </div>
                <button
                  type="button"
                  className="text-button"
                  onClick={() => {
                    setSweepLookback(report.sweep.parameters.lookback_trades);
                    setSweepThreshold(report.sweep.parameters.threshold_ticks);
                    setSweepWindow(report.sweep.parameters.reversion_window_ms);
                  }}
                >
                  Reset parameters
                </button>
              </div>

              <div className="synthetic-chart" aria-hidden="true">
                {SYNTHETIC_TRADES.map((trade) => {
                  const bottom = ((trade.price - 5247) / 6) * 100;
                  return (
                    <i
                      key={trade.index}
                      className={
                        sweepResult.triggers.has(trade.index) ? "is-trigger" : ""
                      }
                      style={{ bottom: `${Math.max(2, Math.min(98, bottom))}%` }}
                    />
                  );
                })}
              </div>

              <div className="lab-result" aria-live="polite">
                <strong>{sweepResult.total}</strong>
                <span>
                  synthetic events ({sweepResult.above} above, {sweepResult.below}{" "}
                  below)
                </span>
              </div>

              <div className="slider-grid">
                <label>
                  <span>
                    Lookback <output>{sweepLookback} trades</output>
                  </span>
                  <input
                    type="range"
                    min="10"
                    max="50"
                    step="5"
                    value={sweepLookback}
                    onChange={(event) =>
                      setSweepLookback(Number(event.target.value))
                    }
                  />
                </label>
                <label>
                  <span>
                    Penetration <output>{sweepThreshold} ticks</output>
                  </span>
                  <input
                    type="range"
                    min="2"
                    max="8"
                    value={sweepThreshold}
                    onChange={(event) =>
                      setSweepThreshold(Number(event.target.value))
                    }
                  />
                </label>
                <label>
                  <span>
                    Reversion window{" "}
                    <output>{formatInteger.format(sweepWindow)} ms</output>
                  </span>
                  <input
                    type="range"
                    min="1000"
                    max="7000"
                    step="500"
                    value={sweepWindow}
                    onChange={(event) =>
                      setSweepWindow(Number(event.target.value))
                    }
                  />
                </label>
              </div>
            </article>
          </div>

          <p className="signal-caveat">
            This is a documented market-data heuristic, not a trading signal,
            execution recommendation, hidden-liquidity estimate, or P&amp;L
            claim.
          </p>
        </section>

        <section id="benchmarks" className="story-section" data-story-step>
          <SectionHeader
            step="07"
            kicker="Complete measured matrix"
            title="Explore speed, memory, and explicit limits."
          >
            <p>
              Thirty-one configurations were measured in fresh child
              processes. One unsafe high-memory plan was rejected before
              allocation and remains visible as an unsupported result.
            </p>
          </SectionHeader>

          <div className="benchmark-controls">
            <label>
              Schema
              <select
                value={benchmarkSchema}
                onChange={(event) => setBenchmarkSchema(event.target.value)}
              >
                <option value="all">All schemas</option>
                {Array.from(
                  new Set(report.benchmark.results.map((row) => row.schema)),
                ).map((schema) => (
                  <option value={schema} key={schema}>
                    {schema}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Compression
              <select
                value={benchmarkCompression}
                onChange={(event) => setBenchmarkCompression(event.target.value)}
              >
                <option value="all">All</option>
                <option value="zstd">Zstd</option>
                <option value="none">Raw</option>
              </select>
            </label>
            <label>
              Access
              <select
                value={benchmarkAccess}
                onChange={(event) => setBenchmarkAccess(event.target.value)}
              >
                <option value="all">All</option>
                <option value="streaming">Streaming</option>
                <option value="fully_buffered_input">Fully buffered</option>
              </select>
            </label>
            <label>
              Concurrency
              <select
                value={benchmarkConcurrency}
                onChange={(event) =>
                  setBenchmarkConcurrency(event.target.value)
                }
              >
                <option value="all">All</option>
                <option value="single_thread">Single thread</option>
                <option value="parallel_independent_streams">
                  4 independent streams
                </option>
              </select>
            </label>
            <label>
              Metric
              <select
                value={benchmarkMetric}
                onChange={(event) =>
                  setBenchmarkMetric(event.target.value as BenchmarkMetric)
                }
              >
                {BENCHMARK_METRICS.map((metric) => (
                  <option value={metric.key} key={metric.key}>
                    {metric.label}
                  </option>
                ))}
              </select>
            </label>
          </div>

          <div className="benchmark-summary">
            <span>
              Showing <strong>{filteredBenchmarks.length}</strong> measured rows
            </span>
            <span>
              Metric:{" "}
              <strong>
                {
                  BENCHMARK_METRICS.find(
                    (metric) => metric.key === benchmarkMetric,
                  )?.label
                }
              </strong>
            </span>
          </div>

          <div className="benchmark-views">
            <div className="benchmark-bars" aria-label="Filtered benchmark bars">
              {filteredBenchmarks.map((row) => (
                <article
                  key={`${row.schema}-${row.compression}-${row.access}-${row.concurrency}`}
                >
                  <div>
                    <span className="mono-chip">{row.schema}</span>
                    <strong>
                      {formatBenchmarkValue(row, benchmarkMetric)}{" "}
                      <small>
                        {
                          BENCHMARK_METRICS.find(
                            (metric) => metric.key === benchmarkMetric,
                          )?.short
                        }
                      </small>
                    </strong>
                  </div>
                  <p>
                    {humanize(row.compression)} · {humanize(row.access)} ·{" "}
                    {row.concurrency === "parallel_independent_streams"
                      ? "4 independent streams"
                      : "single thread"}
                  </p>
                  <span className="benchmark-bar-track" aria-hidden="true">
                    <i
                      style={{
                        width: `${Math.max(
                          1,
                          (row[benchmarkMetric] / benchmarkMax) * 100,
                        )}%`,
                      }}
                    />
                  </span>
                </article>
              ))}
            </div>

            <div className="scatter-card">
              <div className="scatter-heading">
                <div>
                  <p className="eyebrow">Tradeoff map</p>
                  <h3>Throughput vs. peak memory</h3>
                </div>
                <span>Filtered rows</span>
              </div>
              <div
                className="scatter-plot"
                aria-hidden="true"
              >
                <span className="axis-y">More throughput ↑</span>
                <span className="axis-x">More memory →</span>
                {filteredBenchmarks.map((row) => (
                  <span
                    key={`scatter-${row.schema}-${row.compression}-${row.access}-${row.concurrency}`}
                    className={`scatter-dot schema-${row.schema.replaceAll("-", "")}`}
                    style={{
                      left: `${Math.min(
                        96,
                        Math.max(
                          3,
                          (row.peak_rss_mib_median / maxMemory) * 92 + 2,
                        ),
                      )}%`,
                      bottom: `${Math.min(
                        94,
                        Math.max(
                          4,
                          (row.messages_per_second_median / maxThroughput) * 88 +
                            3,
                        ),
                      )}%`,
                    }}
                    title={`${row.schema}: ${humanize(row.access)}`}
                  />
                ))}
              </div>
            </div>
          </div>

          <div className="table-scroll" tabIndex={0}>
            <table>
              <caption>Filtered benchmark results</caption>
              <thead>
                <tr>
                  <th>Schema</th>
                  <th>Compression</th>
                  <th>Access</th>
                  <th>Concurrency</th>
                  <th>Median msg/s</th>
                  <th>Decoded MiB/s</th>
                  <th>ns/msg</th>
                  <th>Peak RSS MiB</th>
                </tr>
              </thead>
              <tbody>
                {filteredBenchmarks.map((row) => (
                  <tr
                    key={`table-${row.schema}-${row.compression}-${row.access}-${row.concurrency}`}
                  >
                    <td>{row.schema}</td>
                    <td>{row.compression}</td>
                    <td>{humanize(row.access)}</td>
                    <td>
                      {row.concurrency === "parallel_independent_streams"
                        ? "4 independent streams"
                        : "single thread"}
                    </td>
                    <td>
                      {formatInteger.format(row.messages_per_second_median)}
                    </td>
                    <td>
                      {formatDecimal.format(
                        row.decoded_mib_per_second_median,
                      )}
                    </td>
                    <td>
                      {formatDecimal.format(
                        row.nanoseconds_per_message_median,
                      )}
                    </td>
                    <td>{formatDecimal.format(row.peak_rss_mib_median)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="capability-grid">
            {report.benchmark.capabilities.map((capability) => (
              <article key={capability.feature}>
                <span
                  className={`capability-status status-${capability.status.replaceAll(
                    "_",
                    "-",
                  )}`}
                >
                  {humanize(capability.status)}
                </span>
                <h3>{humanize(capability.feature)}</h3>
                <p>{capability.detail}</p>
              </article>
            ))}
          </div>

          {report.benchmark.unsupported_configurations.map((configuration) => (
            <article
              className="unsupported-card"
              key={`${configuration.schema}-${configuration.access}-${configuration.concurrency}`}
            >
              <EvidenceBadge status="unsupported" />
              <div>
                <p className="eyebrow">
                  {configuration.schema} · {humanize(configuration.access)}
                </p>
                <h3>Rejected before allocation</h3>
                <p>{configuration.reason}</p>
              </div>
            </article>
          ))}
        </section>

        <section id="parity" className="story-section" data-story-step>
          <SectionHeader
            step="08"
            kicker="One core, two interfaces"
            title="Rust owns the truth. Node proves the boundary."
          >
            <p>
              The native binding does not duplicate market logic. It converts
              owned values at the N-API edge and preserves integers that exceed
              JavaScript&apos;s safe-number range as bigint.
            </p>
          </SectionHeader>

          <div className="parity-diagram">
            <article>
              <span className="language-mark">RS</span>
              <p className="eyebrow">dbn-es-core</p>
              <h3>Decoder + state machines</h3>
              <ul>
                <li>Typed streaming records</li>
                <li>Snapshot-gated MBO book</li>
                <li>Parameterized sweep detector</li>
              </ul>
            </article>
            <div className="parity-link" aria-label="Shared native boundary">
              <span>{formatInteger.format(report.parity.mbo_records)}</span>
              <small>matching MBO records</small>
              <i aria-hidden="true">↔</i>
              <span>{report.parity.event_count}</span>
              <small>matching events</small>
            </div>
            <article>
              <span className="language-mark">JS</span>
              <p className="eyebrow">dbn-es-node</p>
              <h3>N-API streaming interface</h3>
              <ul>
                <li>Generated TypeScript declarations</li>
                <li>Bigint-safe 64-bit fields</li>
                <li>Cross-language parity test</li>
              </ul>
            </article>
          </div>

          <div className="command-grid">
            <article>
              <div>
                <p className="eyebrow">Clean checkout</p>
                <h3>Verify synthetic evidence</h3>
              </div>
              <pre>
                <code>.\scripts\verify.ps1</code>
              </pre>
              <button
                type="button"
                className="copy-button"
                onClick={() =>
                  copyText(".\\scripts\\verify.ps1", "verification command")
                }
              >
                Copy command
              </button>
            </article>
            <article>
              <div>
                <p className="eyebrow">Restored corpus</p>
                <h3>Regenerate live evidence</h3>
              </div>
              <pre>
                <code>./scripts/verify.sh --full</code>
              </pre>
              <button
                type="button"
                className="copy-button"
                onClick={() =>
                  copyText(
                    "./scripts/verify.sh --full",
                    "full verification command",
                  )
                }
              >
                Copy command
              </button>
            </article>
          </div>

          <aside className="method-note">
            <span className="method-glyph" aria-hidden="true">
              ✓
            </span>
            <p>
              The fast path creates deterministic four-schema fixtures,
              validates reconstruction, runs the heuristic, tests malformed
              input, and audits generated evidence. It never acquires data or
              spends money.
            </p>
          </aside>
        </section>

        <section id="limits" className="story-section final-section" data-story-step>
          <SectionHeader
            step="09"
            kicker="Evidence before claims"
            title="What this project proves—and what it does not."
          >
            <p>
              Strong engineering is explicit about the edge of the evidence.
              These boundaries are part of the result, not fine print.
            </p>
          </SectionHeader>

          <div className="limitations-grid">
            {report.limitations.map((limitation, index) => (
              <article key={limitation}>
                <span>{String(index + 1).padStart(2, "0")}</span>
                <p>{limitation}</p>
              </article>
            ))}
          </div>

          <div className="final-cta">
            <div>
              <p className="eyebrow">Portfolio case study</p>
              <h2>Built to make correctness inspectable.</h2>
              <p>
                The source package includes the strict Rust core, Node binding,
                deterministic fixtures, benchmark harness, evidence generators,
                recovery boundaries, and one-command verification.
              </p>
            </div>
            <div className="final-actions">
              <button
                type="button"
                className="button button-primary"
                onClick={() => window.print()}
              >
                Print case study
              </button>
              <a className="button button-secondary" href="#overview">
                Back to overview
              </a>
            </div>
          </div>

          <footer>
            <div>
              <span className="wordmark">DBN/ES</span>
              <p>
                Generated from reviewed aggregate evidence. No raw purchased
                records, remote fonts, telemetry, cookies, or runtime market
                requests.
              </p>
            </div>
            <div>
              <a href="#data">Data</a>
              <a href="#validation">Validation</a>
              <a href="#benchmarks">Benchmarks</a>
              <a href="#limits">Limitations</a>
            </div>
          </footer>
        </section>
      </main>

      <SourceDialog claim={sourceClaim} onClose={() => setSourceClaim(null)} />
      <p className="sr-only" aria-live="polite">
        {copyMessage}
      </p>
    </div>
  );
}
