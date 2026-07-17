$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
$remaps = @(
    @($env:USERPROFILE, "/build/user"),
    @($cargoHome, "/build/cargo"),
    @($repoRoot, "/workspace/vector-lsp")
)
$flags = [System.Collections.Generic.List[string]]::new()
$flags.Add("-ladvapi32")
foreach ($entry in $remaps) {
    if (-not [string]::IsNullOrWhiteSpace($entry[0])) {
        $flags.Add("--remap-path-prefix=$($entry[0])=$($entry[1])")
    }
}

$previous = $env:CARGO_ENCODED_RUSTFLAGS
try {
    $env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]0x1f
    & cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw "vector-lsp release build failed with exit code $LASTEXITCODE." }
}
finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $previous
}
