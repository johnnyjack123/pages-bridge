<#
.SYNOPSIS
    Makes a double-click on a .pages file convert it to .docx and open it in WordCraft.
.DESCRIPTION
    Registers a per-user file type (HKCU only, no admin rights needed).
    Run with -Uninstall to remove the association again.
#>
param([switch]$Uninstall)

$ErrorActionPreference = 'Stop'
$progId = 'Pages2Docx.Document'
$classes = 'HKCU:\Software\Classes'

function Notify-Explorer {
    # Explorer caches associations until it receives SHCNE_ASSOCCHANGED.
    Add-Type -Namespace Win32 -Name Shell -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("shell32.dll")]
public static extern void SHChangeNotify(int eventId, int flags, System.IntPtr item1, System.IntPtr item2);
'@
    [Win32.Shell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
}

function Ensure-Key([string]$path) {
    # New-Item -Force would wipe an existing key's values, so only create missing keys.
    if (-not (Test-Path $path)) { New-Item -Path $path -Force | Out-Null }
}

if ($Uninstall) {
    if (Test-Path "$classes\$progId") { Remove-Item "$classes\$progId" -Recurse -Force }
    if (Test-Path "$classes\.pages\OpenWithProgids") {
        Remove-ItemProperty "$classes\.pages\OpenWithProgids" -Name $progId -ErrorAction SilentlyContinue
    }
    if ((Test-Path "$classes\.pages") -and (Get-Item "$classes\.pages").GetValue('') -eq $progId) {
        Set-Item "$classes\.pages" -Value ''
    }
    Notify-Explorer
    Write-Host 'Dateizuordnung für .pages entfernt.'
    return
}

$pyw = (Get-Command pyw.exe -ErrorAction SilentlyContinue).Source
if (-not $pyw) { throw 'pyw.exe (Python Launcher) wurde nicht gefunden.' }
$script = Join-Path $PSScriptRoot 'pages2docx.py'
$wordcraft = 'C:\Program Files\ArtCraft\WordCraft\wordcraft.exe'

Ensure-Key "$classes\$progId\shell\open\command"
Set-Item "$classes\$progId" -Value 'Apple Pages-Dokument'
Set-Item "$classes\$progId\shell\open\command" -Value "`"$pyw`" -3 `"$script`" --open `"%1`""
if (Test-Path $wordcraft) {
    Ensure-Key "$classes\$progId\DefaultIcon"
    Set-Item "$classes\$progId\DefaultIcon" -Value "`"$wordcraft`",0"
}
Ensure-Key "$classes\.pages\OpenWithProgids"
Set-Item "$classes\.pages" -Value $progId
New-ItemProperty "$classes\.pages\OpenWithProgids" -Name $progId -Value '' -PropertyType String -Force | Out-Null
Notify-Explorer
Write-Host 'Fertig: Doppelklick auf eine .pages-Datei öffnet sie jetzt in WordCraft.'
