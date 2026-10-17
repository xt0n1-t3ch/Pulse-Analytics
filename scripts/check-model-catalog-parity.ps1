[CmdletBinding()]
param([string]$UpstreamRoot)

$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($UpstreamRoot)) {
    $UpstreamRoot = Join-Path $PSScriptRoot '../../Codex-Discord-Rich-Presence'
}
$local = Join-Path $PSScriptRoot '../src/codex/model_catalog.json'
$upstream = Join-Path $UpstreamRoot 'src/model_catalog.json'
if ((Get-FileHash -LiteralPath $local).Hash -ne (Get-FileHash -LiteralPath $upstream).Hash) {
    throw 'Model catalog parity failed: Pulse differs from the canonical runtime catalog.'
}
$catalog = Get-Content -LiteralPath $local -Raw | ConvertFrom-Json
$astra = @($catalog.models | Where-Object id -eq 'gpt-6-astra')
if ($astra.Count -ne 1 -or $astra[0].display_name -ne 'GPT-6 Astra' -or $astra[0].context.max_input_tokens -ne 922000) {
    throw 'Astra catalog contract failed.'
}
$sol = @($catalog.models | Where-Object id -eq 'gpt-6.1-sol')
if ($sol.Count -ne 1 -or $sol[0].display_name -ne 'GPT-6.1 Sol' -or
    $sol[0].context.raw_tokens -ne 500000 -or $sol[0].context.effective_percent -ne 95 -or
    $sol[0].context.long_context_input_threshold -ne 272000 -or
    $sol[0].fast_usage_multiplier -ne 2.0 -or $sol[0].api_pricing.input_per_million -ne 2.0 -or
    $sol[0].api_pricing.cached_input_per_million -ne 0.1 -or $sol[0].api_pricing.output_per_million -ne 10.0 -or
    $sol[0].long_context_pricing.input_and_cache -ne 2.0 -or $sol[0].long_context_pricing.output -ne 1.5 -or
    ($sol[0].reasoning_efforts -join ',') -ne 'low,medium,high,xhigh,max,ultra') {
    throw 'GPT-6.1 Sol catalog contract failed.'
}
Write-Output 'MODEL CATALOG PARITY OK'
