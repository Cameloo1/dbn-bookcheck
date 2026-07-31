param(
    [switch]$Full
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ProjectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$SampleDirectory = Join-Path $ProjectRoot "data\sample"
$SampleManifest = Join-Path $SampleDirectory "manifest.json"

function Invoke-Checked {
    param(
        [Parameter(Mandatory)] [string]$Executable,
        [Parameter(Mandatory)] [string[]]$Arguments
    )
    & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Executable exited with status $LASTEXITCODE"
    }
}

Push-Location $ProjectRoot
try {
    Write-Host "==> Rust quality gate"
    Invoke-Checked cargo @("fmt", "--all", "--check")
    Invoke-Checked cargo @("check", "--workspace", "--all-targets", "--locked")
    $env:PROPTEST_CASES = "256"
    Invoke-Checked cargo @("test", "--workspace", "--all-targets", "--locked")
    Invoke-Checked cargo @("clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings")
    $PriorRustdocFlags = $env:RUSTDOCFLAGS
    $env:RUSTDOCFLAGS = "-D warnings"
    try {
        Invoke-Checked cargo @("doc", "--workspace", "--no-deps", "--locked")
    }
    finally {
        $env:RUSTDOCFLAGS = $PriorRustdocFlags
    }

    Write-Host "==> Deterministic synthetic fixture"
    Invoke-Checked cargo @("run", "--release", "--locked", "-p", "dbn-es-bench", "--bin", "dbn-es-bench", "--", "data", "sample", "--output-dir", $SampleDirectory)
    Invoke-Checked cargo @("run", "--release", "--locked", "-p", "dbn-es-bench", "--bin", "dbn-es-bench", "--", "decode", "stats", "--manifest", $SampleManifest, "--output", (Join-Path $SampleDirectory "decode-stats.json"))
    Invoke-Checked cargo @("run", "--release", "--locked", "-p", "dbn-es-bench", "--bin", "dbn-es-bench", "--", "analyze", "validate", "--manifest", $SampleManifest, "--output", (Join-Path $SampleDirectory "book-validation.md"), "--json-output", (Join-Path $SampleDirectory "book-validation.json"))
    Invoke-Checked cargo @("run", "--release", "--locked", "-p", "dbn-es-bench", "--bin", "dbn-es-bench", "--", "analyze", "sweeps", "--manifest", $SampleManifest, "--config", (Join-Path $ProjectRoot "config\sweep.json"), "--output", (Join-Path $SampleDirectory "sweeps.jsonl"), "--summary", (Join-Path $SampleDirectory "sweep-summary.json"), "--report", (Join-Path $SampleDirectory "sweep-detection.md"))

    if ($Full) {
        $LiveManifest = Join-Path $ProjectRoot "data\manifest.json"
        $LiveSessionConfig = Join-Path $ProjectRoot "config\live-session.json"
        if (-not (Test-Path -LiteralPath $LiveManifest -PathType Leaf) -or -not (Test-Path -LiteralPath $LiveSessionConfig -PathType Leaf)) {
            throw "full verification requires operator-restored data/manifest.json, config/live-session.json, and the referenced DBN files"
        }
        Write-Host "==> Verified live corpus and promoted reports"
        Invoke-Checked cargo @("run", "--release", "--locked", "-p", "dbn-es-bench", "--bin", "dbn-es-bench", "--", "data", "verify", "--config", $LiveSessionConfig)
        Invoke-Checked cargo @("run", "--release", "--locked", "-p", "dbn-es-bench", "--bin", "dbn-es-bench", "--", "decode", "stats", "--manifest", $LiveManifest, "--output", (Join-Path $ProjectRoot "data\decode-stats.json"))
        Invoke-Checked cargo @("run", "--release", "--locked", "-p", "dbn-es-bench", "--bin", "dbn-es-bench", "--", "analyze", "validate", "--manifest", $LiveManifest, "--output", (Join-Path $ProjectRoot "docs\book-validation.md"), "--json-output", (Join-Path $ProjectRoot "data\book-validation.json"))
        Invoke-Checked cargo @("run", "--release", "--locked", "-p", "dbn-es-bench", "--bin", "dbn-es-bench", "--", "analyze", "sweeps", "--manifest", $LiveManifest, "--config", (Join-Path $ProjectRoot "config\sweep.json"), "--output", (Join-Path $ProjectRoot "out\sweeps.jsonl"), "--summary", (Join-Path $ProjectRoot "data\sweep-summary.json"), "--report", (Join-Path $ProjectRoot "docs\sweep-detection.md"))
        Invoke-Checked cargo @("run", "--release", "--locked", "-p", "dbn-es-bench", "--bin", "dbn-es-benchmark", "--", "run", "--manifest", $LiveManifest, "--uncompressed-dir", (Join-Path $ProjectRoot "data\uncompressed"), "--output", (Join-Path $ProjectRoot "bench\results.json"))
    }

    Write-Host "==> Generated public reports"
    $BenchmarkResults = Join-Path $ProjectRoot "bench\results.json"
    $BenchmarkMachine = Join-Path $ProjectRoot "bench\machine.json"
    $PublicGenerationInputs = @(
        $BenchmarkResults,
        $BenchmarkMachine,
        (Join-Path $ProjectRoot "config\live-session.json"),
        (Join-Path $ProjectRoot "evidence\public\acquisition-summary.json"),
        (Join-Path $ProjectRoot "evidence\public\book-validation-summary.json"),
        (Join-Path $ProjectRoot "evidence\public\parity-summary.json"),
        (Join-Path $ProjectRoot "evidence\public\sweep-summary.json")
    )
    if (($PublicGenerationInputs | Where-Object { -not (Test-Path -LiteralPath $_ -PathType Leaf) }).Count -eq 0) {
        Invoke-Checked node @("scripts/generate-bench-report.mjs", "bench/results.json", "bench/results.md")
        Invoke-Checked node @("scripts/generate-readme.mjs")
        Invoke-Checked node @("scripts/generate-presentation.mjs")
        Invoke-Checked node @("scripts/generate-public-report-data.mjs", "--check")
    }
    else {
        Write-Host "local evidence inputs are intentionally absent; validating promoted public reports"
    }
    Invoke-Checked node @("scripts/validate-results-html.mjs", "docs/results.html")
    Invoke-Checked node @("scripts/validate-public-report-data.mjs")
    Invoke-Checked node @("scripts/audit-repository.mjs")

    Write-Host "==> Static portfolio"
    Push-Location (Join-Path $ProjectRoot "web")
    try {
        if (Test-Path -LiteralPath "package-lock.json" -PathType Leaf) {
            Invoke-Checked npm.cmd @("ci")
        }
        else {
            Invoke-Checked npm.cmd @("install", "--no-package-lock")
        }
        Invoke-Checked npm.cmd @("run", "verify")
    }
    finally {
        Pop-Location
    }
    Invoke-Checked node @("scripts/audit-web-output.mjs", "web/dist")

    Write-Host "==> Node/TypeScript package"
    Push-Location (Join-Path $ProjectRoot "node")
    try {
        if (Test-Path -LiteralPath "package-lock.json" -PathType Leaf) {
            Invoke-Checked npm.cmd @("ci")
        }
        else {
            Invoke-Checked npm.cmd @("install", "--no-package-lock")
        }
        Invoke-Checked npm.cmd @("run", "build:native")
        Invoke-Checked npm.cmd @("run", "typecheck")
        Invoke-Checked npm.cmd @("run", "example", "--", $SampleManifest)
        Invoke-Checked npm.cmd @("run", "pack:check")
        if ($Full) {
            Invoke-Checked npm.cmd @("run", "parity", "--", (Join-Path $ProjectRoot "data\manifest.json"))
        }
    }
    finally {
        Pop-Location
    }

    Write-Host "verification passed (sample provenance: synthetic; full live evidence: $($Full.IsPresent))"
}
finally {
    Pop-Location
}
