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

New-Item -ItemType Directory -Force -Path $KnotDataDir | Out-Null

if (Get-Command claude -ErrorAction SilentlyContinue) {
    Write-Host "Registering with Claude Code..."
    claude mcp add -s user -e "KNOT_DATA_DIR=$KnotDataDir" $McpName -- $Bin
}
if (Get-Command codex -ErrorAction SilentlyContinue) {
    Write-Host "Registering with Codex..."
    codex mcp add $McpName -- $Bin
}

Write-Host ""
Write-Host "Other agents: Knot is a stdio MCP server; point any client at the binary:"
Write-Host "  `"mcpServers`": { `"$McpName`": { `"command`": `"$($Bin -replace '\\','\\')`" } }"
Write-Host ""
Write-Host "Next, in each repository: knot init"
