param(
    # Only passed on to Claude Code when given; Knot defaults to ~\.knot on its own
    [string]$KnotDataDir
)

$ErrorActionPreference = "Stop"
$McpName = "knot"

# Char codes, not literals: Windows PowerShell 5.1 reads a BOM-less script as ANSI
$Check = [char]0x2713
$Dot = [char]0x00B7
$Cross = [char]0x2717

function Show-Done([string]$Label, [string]$Detail) { Write-Host ("{0} {1,-12}  {2}" -f $Check, $Label, $Detail) }
function Show-Note([string]$Label, [string]$Detail) { Write-Host ("{0} {1,-12}  {2}" -f $Dot, $Label, $Detail) }
function Show-Fail([string]$Label, [string]$Detail) { Write-Host ("{0} {1,-12}  {2}" -f $Cross, $Label, $Detail) -ForegroundColor Red }
function Get-Tilde([string]$Path) { $Path.Replace($env:USERPROFILE, "~") }

# Run a native command with all its output dropped; true when it exits 0. Under "Stop",
# Windows PowerShell 5.1 turns a redirected stderr line into a terminating error, so the
# preference is relaxed for the call.
function Invoke-Quiet([scriptblock]$Command) {
    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $Command *> $null
        return ($LASTEXITCODE -eq 0)
    } finally {
        $ErrorActionPreference = $previous
    }
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Show-Fail "binary" "cargo is missing: https://rustup.rs"
    exit 1
}

Write-Host "knot $Dot building from $(Get-Tilde (Get-Location).Path)"
Write-Host ""

if (-not (Invoke-Quiet { cargo build --release --quiet })) {
    Show-Fail "binary" "cargo build --release failed; run it to see why"
    exit 1
}
$Bin = Join-Path (Get-Location) "target\release\knot.exe"
Show-Done "binary" (Get-Tilde $Bin)

if (Get-Command claude -ErrorAction SilentlyContinue) {
    $extra = @()
    if ($KnotDataDir) { $extra = @("-e", "KNOT_DATA_DIR=$KnotDataDir") }
    if (Invoke-Quiet { claude mcp get $McpName }) {
        Show-Note "claude" "MCP server '$McpName' already registered"
    } elseif (Invoke-Quiet { claude mcp add -s user @extra $McpName -- $Bin }) {
        Show-Done "claude" "MCP server added (user scope)"
    } else {
        Show-Fail "claude" "registration failed; run: claude mcp add -s user $McpName -- `"$Bin`""
    }
}

if (Get-Command codex -ErrorAction SilentlyContinue) {
    if (Invoke-Quiet { codex mcp add $McpName -- $Bin }) {
        Show-Done "codex" "MCP server added"
    } else {
        Show-Fail "codex" "registration failed; add [mcp_servers.$McpName] command = `"$Bin`" to ~\.codex\config.toml"
    }
}

$JsonBin = $Bin -replace '\\', '\\'
Show-Note "other agents" "any stdio MCP client works; point it at the binary:"
Write-Host "                  `"mcpServers`": { `"$McpName`": { `"command`": `"$JsonBin`" } }"
Show-Note "next" "run knot init in each repo: pick your agents, then it indexes the code into .knot\cache\"
