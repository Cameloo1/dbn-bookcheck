# DBN/ES interactive case study

This directory contains the static GitHub Pages presentation for
`dbn-es-bench`. It renders reviewed aggregate evidence and deterministic
synthetic teaching sequences. It never reads purchased DBN payloads,
credentials, request ledgers, or runtime market data.

## Local development

Prerequisites: Node.js 22.13 or newer.

```sh
npm install --no-package-lock
npm run dev
```

The application reads `public/data/report.v1.json`. Regenerate and validate
that file from the repository root:

```sh
node scripts/generate-public-report-data.mjs
node scripts/validate-public-report-data.mjs
```

## Verification

```sh
npm run verify
npm run test:e2e
```

`npm run verify` type-checks, lints, builds, and audits the static contract.
Playwright provides the separate interaction, browser, console-error, and
accessibility gate.

## GitHub Pages

The repository workflow builds the site with the repository-name base path,
audits the exact `dist/` directory, and packages a Pages artifact. Ordinary
pushes only validate the artifact. Deployment runs only from a manually
dispatched workflow after owner approval.

Generated `dist/`, browser traces, screenshots, coverage, dependencies, local
logs, paid payloads, and internal audit notes remain outside the publication
set.
