param(
    [string]$ReferenceSourceRoot = $env:REFERENCE_TXT_SOURCE_ROOT
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($ReferenceSourceRoot)) {
    throw "Reference source root is required. Pass -ReferenceSourceRoot explicitly or set REFERENCE_TXT_SOURCE_ROOT."
}

$datasets = @(
    [ordered]@{ schemaVariant = "1.13"; gameVersion = "1.13c"; datasetId = "113c" },
    [ordered]@{ schemaVariant = "2.4"; gameVersion = "2.4"; datasetId = "69270" },
    [ordered]@{ schemaVariant = "3.1"; gameVersion = "3.1"; datasetId = "92198" },
    [ordered]@{ schemaVariant = "3.2"; gameVersion = "3.2"; datasetId = "92777a" }
)

$contribRoot = $PSScriptRoot
$strictUtf8 = New-Object System.Text.UTF8Encoding($false, $true)

function Get-Sha256Hex([byte[]]$Bytes) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($sha.ComputeHash($Bytes))).Replace("-", "").ToLowerInvariant()
    }
    finally {
        $sha.Dispose()
    }
}

function Get-Utf8Bytes([string]$Text) {
    return (New-Object System.Text.UTF8Encoding($false)).GetBytes($Text)
}

function Get-DetectedEncoding([byte[]]$Bytes) {
    try {
        $null = $strictUtf8.GetString($Bytes)
        return "utf-8"
    }
    catch [System.Text.DecoderFallbackException] {
        return "windows-1252"
    }
}

$manifestDatasets = @()
$rootLines = New-Object System.Collections.Generic.List[string]

foreach ($dataset in $datasets) {
    $sourceDir = Join-Path $ReferenceSourceRoot $dataset.datasetId
    if (-not (Test-Path -LiteralPath $sourceDir -PathType Container)) {
        throw "Reference dataset source directory is missing: $sourceDir"
    }

    $resourceRelative = "$($dataset.schemaVariant)/reference"
    $destinationDir = Join-Path $contribRoot ($resourceRelative.Replace("/", [System.IO.Path]::DirectorySeparatorChar))
    [System.IO.Directory]::CreateDirectory($destinationDir) | Out-Null

    $files = @()
    $datasetLines = New-Object System.Collections.Generic.List[string]
    $totalBytes = [int64]0
    $sourceFiles = @(Get-ChildItem -LiteralPath $sourceDir -File -Filter "*.txt" | Sort-Object { $_.Name.ToLowerInvariant() }, Name)
    $sourceNames = @{}
    foreach ($sourceFile in $sourceFiles) {
        $sourceNames[$sourceFile.Name.ToLowerInvariant()] = $true
    }
    foreach ($existingFile in @(Get-ChildItem -LiteralPath $destinationDir -File)) {
        if (-not $sourceNames.ContainsKey($existingFile.Name.ToLowerInvariant())) {
            Remove-Item -LiteralPath $existingFile.FullName -Force
        }
    }

    foreach ($sourceFile in $sourceFiles) {
        $bytes = [System.IO.File]::ReadAllBytes($sourceFile.FullName)
        $destination = Join-Path $destinationDir $sourceFile.Name
        [System.IO.File]::WriteAllBytes($destination, $bytes)

        $hash = Get-Sha256Hex $bytes
        $relativePath = $sourceFile.Name.Replace("\", "/")
        $lowerPath = $relativePath.ToLowerInvariant()
        $size = [int64]$bytes.LongLength
        $totalBytes += $size
        $datasetLines.Add("$hash  $lowerPath")
        $rootLines.Add("$hash  $($dataset.gameVersion)/$lowerPath")
        $files += [ordered]@{
            path = $relativePath
            bytes = $size
            encoding = Get-DetectedEncoding $bytes
            sha256 = $hash
        }
    }

    $datasetCanonical = (($datasetLines -join "`n") + "`n")
    $manifestDatasets += [ordered]@{
        schemaVariant = $dataset.schemaVariant
        gameVersion = $dataset.gameVersion
        source = [ordered]@{
            datasetId = $dataset.datasetId
        }
        resourcePath = $resourceRelative
        fileCount = $files.Count
        totalBytes = $totalBytes
        canonicalSha256 = Get-Sha256Hex (Get-Utf8Bytes $datasetCanonical)
        files = $files
    }
}

$rootCanonical = (($rootLines -join "`n") + "`n")
$allFileCount = [int64]0
$allBytes = [int64]0
foreach ($dataset in $manifestDatasets) {
    $allFileCount += [int64]$dataset["fileCount"]
    $allBytes += [int64]$dataset["totalBytes"]
}

$manifest = [ordered]@{
    formatVersion = 1
    generatedAtUtc = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    provenance = "Bundled versioned reference TXT bytes; runtime never reads external source directories."
    canonicalHashFormat = "SHA-256 of UTF-8 (no BOM) lines: <file-sha256><two spaces><lowercase-relative-path><LF>, sorted by lowercase path, including the final LF. Root paths are prefixed with gameVersion/."
    totalFileCount = $allFileCount
    totalBytes = $allBytes
    canonicalSha256 = Get-Sha256Hex (Get-Utf8Bytes $rootCanonical)
    datasets = $manifestDatasets
}

$manifestPath = Join-Path $contribRoot "reference-manifest.json"
$json = $manifest | ConvertTo-Json -Depth 8
[System.IO.File]::WriteAllText($manifestPath, $json + "`n", (New-Object System.Text.UTF8Encoding($false)))

Write-Host "Wrote $($manifest.totalFileCount) files ($($manifest.totalBytes) bytes) and $manifestPath"
foreach ($dataset in $manifestDatasets) {
    Write-Host "$($dataset["gameVersion"]): $($dataset["fileCount"]) files, $($dataset["totalBytes"]) bytes, $($dataset["canonicalSha256"])"
}
Write-Host "root: $($manifest.canonicalSha256)"
