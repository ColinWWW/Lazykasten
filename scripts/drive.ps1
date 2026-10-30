<#
.SYNOPSIS
  Reusable driver for the real Lazykasten window (Milestone 21.4).

  Dot-source this file, then call the step functions:
    . .\scripts\drive.ps1
    Start-LkApp -Exe .\target\release\lazykasten.exe -Vault $vault
    Show-LkApp
    Send-LkKeys "hello"
    Click-Lk 400 300            # client coordinates
    Drag-Lk 400 300 520 360
    Scroll-Lk 400 300 -3
    Wait-Lk 500
    Save-LkShot "01-graph"
    Stop-LkApp

  Every action checks the window has focus first. Screenshots go to
  target/ui-shots/. Can also run a JSON step file directly:
    .\scripts\drive.ps1 -Exe ... -Vault ... -StepsFile steps.json
  Steps: {"op":"show|keys|click|drag|scroll|wait|shot", ...}
#>
param(
    [string]$Exe,
    [string]$Vault,
    [string]$StepsFile
)

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

if (-not ([System.Management.Automation.PSTypeName]'LkWin').Type) {
    Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class LkWin {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, int data, IntPtr extra);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    public delegate bool EnumProc(IntPtr h, IntPtr p);
    public struct RECT { public int Left, Top, Right, Bottom; }
    public static IntPtr FindByPid(uint target) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, p) => {
            uint pid; GetWindowThreadProcessId(h, out pid);
            if (pid == target && IsWindowVisible(h)) { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
"@
}
[LkWin]::SetProcessDPIAware() | Out-Null

$script:LkProc = $null
$script:LkShots = Join-Path $PSScriptRoot "..\target\ui-shots"

function Get-LkHwnd {
    if (-not $script:LkProc) { throw "app not started" }
    $script:LkProc.Refresh()
    if ($script:LkProc.MainWindowHandle -ne [IntPtr]::Zero) { return $script:LkProc.MainWindowHandle }
    return [LkWin]::FindByPid([uint32]$script:LkProc.Id)
}

function Get-LkRect {
    $rc = New-Object LkWin+RECT
    [LkWin]::GetWindowRect((Get-LkHwnd), [ref]$rc) | Out-Null
    return $rc
}

function Assert-LkFocus {
    $hwnd = Get-LkHwnd
    if ($hwnd -eq [IntPtr]::Zero) { throw "no window found" }
    for ($i = 0; $i -lt 20; $i++) {
        if ([LkWin]::GetForegroundWindow() -eq $hwnd) { return }
        [LkWin]::SetForegroundWindow($hwnd) | Out-Null
        Start-Sleep -Milliseconds 150
    }
    throw "window would not take focus"
}

function Start-LkApp {
    param([Parameter(Mandatory)][string]$Exe, [Parameter(Mandatory)][string]$Vault, [switch]$Trace)
    Get-Process lazykasten -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 400
    $env:LAZYKASTEN_VAULT = $Vault
    if ($Trace) { $env:LAZYKASTEN_TRACE = "1" } else { Remove-Item Env:LAZYKASTEN_TRACE -ErrorAction SilentlyContinue }
    $script:LkProc = Start-Process $Exe "--hidden" -PassThru
    # The window starts hidden (not yet visible to EnumWindows); the
    # process being alive is enough here. Show-LkApp finds the window.
    Start-Sleep -Milliseconds 1500
    $script:LkProc.Refresh()
    if ($script:LkProc.HasExited) { throw "app exited at startup" }
}

function Stop-LkApp {
    if ($script:LkProc) { Stop-Process -Id $script:LkProc.Id -Force -ErrorAction SilentlyContinue; $script:LkProc = $null }
}

function Show-LkApp {
    [System.Windows.Forms.SendKeys]::SendWait("%j")
    Start-Sleep -Milliseconds 700
    Assert-LkFocus
}

function Send-LkKeys {
    param([Parameter(Mandatory)][string]$Keys, [int]$AfterMs = 350)
    Assert-LkFocus
    [System.Windows.Forms.SendKeys]::SendWait($Keys)
    Start-Sleep -Milliseconds $AfterMs
}

function Wait-Lk { param([int]$Ms = 500) Start-Sleep -Milliseconds $Ms }

function Click-Lk {
    param([int]$X, [int]$Y, [int]$AfterMs = 400)
    Assert-LkFocus
    $rc = Get-LkRect
    [LkWin]::SetCursorPos($rc.Left + $X, $rc.Top + $Y) | Out-Null
    Start-Sleep -Milliseconds 80
    [LkWin]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 60
    [LkWin]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds $AfterMs
}

function Drag-Lk {
    param([int]$X1, [int]$Y1, [int]$X2, [int]$Y2, [int]$Steps = 14, [int]$AfterMs = 500)
    Assert-LkFocus
    $rc = Get-LkRect
    [LkWin]::SetCursorPos($rc.Left + $X1, $rc.Top + $Y1) | Out-Null
    Start-Sleep -Milliseconds 100
    [LkWin]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
    for ($i = 1; $i -le $Steps; $i++) {
        $x = $rc.Left + $X1 + [int](($X2 - $X1) * $i / $Steps)
        $y = $rc.Top + $Y1 + [int](($Y2 - $Y1) * $i / $Steps)
        [LkWin]::SetCursorPos($x, $y) | Out-Null
        Start-Sleep -Milliseconds 45
    }
    [LkWin]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds $AfterMs
}

function Scroll-Lk {
    param([int]$X, [int]$Y, [int]$Clicks = -3, [int]$AfterMs = 400)
    Assert-LkFocus
    $rc = Get-LkRect
    [LkWin]::SetCursorPos($rc.Left + $X, $rc.Top + $Y) | Out-Null
    [LkWin]::mouse_event(0x0800, 0, 0, $Clicks * 120, [IntPtr]::Zero)
    Start-Sleep -Milliseconds $AfterMs
}

function Save-LkShot {
    param([Parameter(Mandatory)][string]$Name)
    New-Item -ItemType Directory -Force $script:LkShots | Out-Null
    $rc = Get-LkRect
    $w = $rc.Right - $rc.Left; $h = $rc.Bottom - $rc.Top
    $bmp = New-Object Drawing.Bitmap $w, $h
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($rc.Left, $rc.Top, 0, 0, (New-Object Drawing.Size $w, $h))
    $g.Dispose()
    $path = Join-Path $script:LkShots "$Name.png"
    $bmp.Save($path, [Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    return $path
}

if ($StepsFile) {
    Start-LkApp -Exe $Exe -Vault $Vault
    try {
        foreach ($s in (Get-Content $StepsFile -Raw | ConvertFrom-Json)) {
            switch ($s.op) {
                "show"  { Show-LkApp }
                "keys"  { Send-LkKeys $s.keys }
                "click" { Click-Lk $s.x $s.y }
                "drag"  { Drag-Lk $s.x1 $s.y1 $s.x2 $s.y2 }
                "scroll"{ Scroll-Lk $s.x $s.y $s.clicks }
                "wait"  { Wait-Lk $s.ms }
                "shot"  { Save-LkShot $s.name | Out-Null }
            }
        }
    } finally { Stop-LkApp }
}
