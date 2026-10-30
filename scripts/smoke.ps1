<#
.SYNOPSIS
  One-command smoke walk of the main flows on a seeded vault, using
  scripts/drive.ps1 against the real release window. One screenshot per
  step lands in target/ui-shots/ (prefix "smoke-").

.EXAMPLE
  .\scripts\smoke.ps1
#>
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
. (Join-Path $PSScriptRoot "drive.ps1")

$vault = Join-Path $root "target\ui-smoke-vault"
if (Test-Path $vault) { Remove-Item $vault -Recurse -Force }
& (Join-Path $PSScriptRoot "seed_demo.ps1") -Vault $vault | Out-Null

$exe = Join-Path $root "target\release\lazykasten.exe"
Start-LkApp -Exe $exe -Vault $vault
try {
    Show-LkApp
    Save-LkShot "smoke-01-editor" | Out-Null
    Send-LkKeys "tokyo smoke capture"          # typing check: t/o must land
    Save-LkShot "smoke-02-capture" | Out-Null
    Send-LkKeys "%j"                            # hide (saves)
    Wait-Lk 400
    Show-LkApp
    Send-LkKeys "^o"                            # browse: tag list
    Save-LkShot "smoke-03-tags" | Out-Null
    Send-LkKeys "tomato 123"                    # typing check in browser search
    Save-LkShot "smoke-04-search" | Out-Null
    Send-LkKeys "{ESC}"
    Send-LkKeys "^+p"                           # theme picker
    Send-LkKeys "tokyo"                         # typing check in theme picker
    Save-LkShot "smoke-05-themes" | Out-Null
    Send-LkKeys "{ESC}"
    Send-LkKeys "^g"                            # graph
    Wait-Lk 2500
    Save-LkShot "smoke-06-graph" | Out-Null
    Send-LkKeys "^g"                            # back to editor if toggle path
    Write-Host "smoke ok: screenshots in target/ui-shots (smoke-*.png)"
} finally {
    Stop-LkApp
}
