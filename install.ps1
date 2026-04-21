param(
    [string]$KnotDataDir = "$env:USERPROFILE\.knot"
)

$ErrorActionPreference = "Stop"
$McpName = "knot"

Write-Host "Building Knot..."
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "cargo not found. Install Rust from https://rustup.rs"
    exit 1
}

cargo build --release

$Bin = Join-Path (Get-Location) "target\release\knot.exe"

if (-not (Get-Command claude -ErrorAction SilentlyContinue)) {
    Write-Warning "'claude' CLI not found — skipping MCP registration."
    Write-Host ""
    Write-Host "Add Knot manually to ~\.claude\settings.json:"
    Write-Host ""
    Write-Host "  `"mcpServers`": {"
    Write-Host "    `"$McpName`": {"
    Write-Host "      `"command`": `"$Bin`","
    Write-Host "      `"env`": { `"KNOT_DATA_DIR`": `"$KnotDataDir`" }"
    Write-Host "    }"
    Write-Host "  }"
    exit 0
}

New-Item -ItemType Directory -Force -Path $KnotDataDir | Out-Null

Write-Host "Registering with Claude Code..."
claude mcp add $McpName $Bin --scope user -e "KNOT_DATA_DIR=$KnotDataDir"

Write-Host ""
Write-Host "Done. Restart Claude Code, then verify with: claude mcp list"
