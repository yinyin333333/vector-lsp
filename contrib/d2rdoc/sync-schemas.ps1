# Fetch schema JS files from the d2rdoc repository and place them under
# contrib/d2rdoc/.
#
# Usage: .\contrib\d2rdoc\sync-schemas.ps1 [-Branch <branch>]
#
# Layout produced:
#   contrib/d2rdoc/<CurrentVersion>/schema/   <- data/files (latest game version)
#   contrib/d2rdoc/<ver>/schema/              <- data/old/<ver>  (one per old version)
#
# Run this whenever d2rdoc ships updated schema files or when a new game
# version is added.  Stale .js files in the destination are removed before
# copying.
#
# Requirements: git 2.25+ (sparse-checkout support)

param(
    [string]$Branch = "master"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ── Configuration ──────────────────────────────────────────────────────────────

$D2rdocRepo = "https://github.com/eezstreet/d2rdoc.git"

# Version name assigned to data/files (the current/latest schema set).
# Update this when a new game version ships and data/files is bumped.
$CurrentVersion = "3.3"

# ── Paths ──────────────────────────────────────────────────────────────────────

$ContribD2rdoc = $PSScriptRoot                          # contrib/d2rdoc/
$TempDir       = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())

# ── Helpers ────────────────────────────────────────────────────────────────────

function Sync-JsFiles([string]$Src, [string]$Dst) {
    New-Item -ItemType Directory -Force -Path $Dst | Out-Null

    # Remove stale .js files that may no longer exist upstream.
    Get-ChildItem -Path $Dst -Filter "*.js" -File | Remove-Item -Force

    $files = Get-ChildItem -Path $Src -Filter "*.js" -File
    foreach ($f in $files) {
        Copy-Item $f.FullName -Destination $Dst
    }
    Write-Host "  $($files.Count) file(s) -> $Dst"
}

# ── Clone (sparse, shallow) ────────────────────────────────────────────────────

try {
    New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
    $CloneDir = Join-Path $TempDir "d2rdoc"

    Write-Host "Cloning d2rdoc @ $Branch (sparse, depth 1)..."
    git clone --depth 1 --branch $Branch --filter=blob:none --sparse $D2rdocRepo $CloneDir
    if ($LASTEXITCODE -ne 0) { throw "git clone failed" }

    git -C $CloneDir sparse-checkout set data/files data/old
    if ($LASTEXITCODE -ne 0) { throw "git sparse-checkout failed" }

    # ── Sync current version ───────────────────────────────────────────────────

    Write-Host "Syncing data/files -> contrib/d2rdoc/$CurrentVersion/schema/"
    Sync-JsFiles (Join-Path $CloneDir "data\files") (Join-Path $ContribD2rdoc "$CurrentVersion\schema")

    # ── Sync old versions ──────────────────────────────────────────────────────

    $OldDir = Join-Path $CloneDir "data\old"
    if (Test-Path $OldDir) {
        foreach ($verDir in Get-ChildItem -Path $OldDir -Directory) {
            Write-Host "Syncing data/old/$($verDir.Name) -> contrib/d2rdoc/$($verDir.Name)/schema/"
            Sync-JsFiles $verDir.FullName (Join-Path $ContribD2rdoc "$($verDir.Name)\schema")
        }
    }

    Write-Host "Done."
}
finally {
    if (Test-Path $TempDir) {
        Remove-Item -Recurse -Force $TempDir
    }
}
