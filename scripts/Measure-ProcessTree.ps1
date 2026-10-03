[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateRange(1, 2147483647)]
    [int]$RootProcessId,

    [ValidateRange(1, 3600)]
    [int]$DurationSeconds = 60,

    [ValidateRange(100, 60000)]
    [int]$IntervalMilliseconds = 1000,

    [switch]$TrackWindowState,

    [Parameter(Mandatory)]
    [string]$OutputPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($TrackWindowState -and -not ('FerricMemoryWindowState' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class FerricMemoryWindowState {
    [DllImport("user32.dll")]
    private static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsIconic(IntPtr window);
    public static int ForegroundProcessId() {
        uint processId;
        GetWindowThreadProcessId(GetForegroundWindow(), out processId);
        return (int)processId;
    }
}
'@
}

$root = Get-Process -Id $RootProcessId
$rootStarted = $root.StartTime.ToUniversalTime().Ticks
$logicalProcessors = [Environment]::ProcessorCount
$previousCpu = @{}
$previousTime = $null
$rows = [System.Collections.Generic.List[object]]::new()
$timer = [System.Diagnostics.Stopwatch]::StartNew()

while ($timer.Elapsed.TotalSeconds -lt $DurationSeconds) {
    try {
        $currentRoot = Get-Process -Id $RootProcessId -ErrorAction Stop
        if ($currentRoot.StartTime.ToUniversalTime().Ticks -ne $rootStarted) {
            break
        }
    } catch [Microsoft.PowerShell.Commands.ProcessCommandException] {
        break
    }

    $snapshot = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, CreationDate)
    $byParent = @{}
    foreach ($entry in $snapshot) {
        $parent = [int]$entry.ParentProcessId
        if (-not $byParent.ContainsKey($parent)) {
            $byParent[$parent] = [System.Collections.Generic.List[object]]::new()
        }
        $byParent[$parent].Add($entry)
    }

    $pending = [System.Collections.Generic.Queue[int]]::new()
    $included = [System.Collections.Generic.HashSet[int]]::new()
    $pending.Enqueue($RootProcessId)
    while ($pending.Count -gt 0) {
        $processId = $pending.Dequeue()
        if (-not $included.Add($processId)) {
            continue
        }
        if ($byParent.ContainsKey($processId)) {
            foreach ($child in $byParent[$processId]) {
                if ($child.CreationDate.ToUniversalTime().Ticks -ge $rootStarted) {
                    $pending.Enqueue([int]$child.ProcessId)
                }
            }
        }
    }

    $workingSet = [long]0
    $privateBytes = [long]0
    $privateWorkingSet = [long]0
    $privateWorkingSetCount = 0
    $residentCounters = @{}
    foreach ($counter in (Get-CimInstance Win32_PerfRawData_PerfProc_Process -Property IDProcess, WorkingSetPrivate)) {
        $residentCounters[[int]$counter.IDProcess] = [long]$counter.WorkingSetPrivate
    }
    $cpuDelta = 0.0
    $processCount = 0
    $nextCpu = @{}
    foreach ($processId in $included) {
        $process = Get-Process -Id $processId -ErrorAction SilentlyContinue
        if ($null -eq $process) {
            continue
        }
        try {
            $key = '{0}:{1}' -f $process.Id, $process.StartTime.ToUniversalTime().Ticks
            $cpu = $process.TotalProcessorTime.TotalSeconds
            $nextCpu[$key] = $cpu
            if ($previousCpu.ContainsKey($key)) {
                $cpuDelta += [Math]::Max(0, $cpu - $previousCpu[$key])
            }
            $workingSet += $process.WorkingSet64
            $privateBytes += $process.PrivateMemorySize64
            if ($residentCounters.ContainsKey($processId)) {
                $privateWorkingSet += $residentCounters[$processId]
                $privateWorkingSetCount++
            }
            $processCount++
        } catch [System.InvalidOperationException] {
            continue
        } catch [System.ComponentModel.Win32Exception] {
            continue
        }
    }

    $now = $timer.Elapsed.TotalSeconds
    $focused = $null
    $minimized = $null
    if ($TrackWindowState) {
        $foregroundProcessId = [FerricMemoryWindowState]::ForegroundProcessId()
        if ($foregroundProcessId -gt 0) {
            $focused = $foregroundProcessId -eq $RootProcessId
        }
        $currentRoot.Refresh()
        if ($currentRoot.MainWindowHandle -ne [IntPtr]::Zero) {
            $minimized = [FerricMemoryWindowState]::IsIconic($currentRoot.MainWindowHandle)
        }
    }
    $cpuOneCore = $null
    $cpuMachine = $null
    if ($null -ne $previousTime -and $now -gt $previousTime) {
        $cpuOneCore = [Math]::Round(100 * $cpuDelta / ($now - $previousTime), 3)
        $cpuMachine = [Math]::Round($cpuOneCore / $logicalProcessors, 3)
    }
    $rows.Add([pscustomobject]@{
        TimestampUtc = [DateTime]::UtcNow.ToString('o')
        ElapsedSeconds = [Math]::Round($now, 3)
        ProcessCount = $processCount
        Focused = $focused
        Minimized = $minimized
        WorkingSetMiB = [Math]::Round($workingSet / 1MB, 2)
        PrivateWorkingSetMiB = $(if ($privateWorkingSetCount -eq $processCount) {
            [Math]::Round($privateWorkingSet / 1MB, 2)
        } else { $null })
        PrivateBytesMiB = [Math]::Round($privateBytes / 1MB, 2)
        CpuPercentOneCore = $cpuOneCore
        CpuPercentMachine = $cpuMachine
    })
    $previousCpu = $nextCpu
    $previousTime = $now

    $remainingMilliseconds = ($DurationSeconds - $timer.Elapsed.TotalSeconds) * 1000
    if ($remainingMilliseconds -gt 0) {
        Start-Sleep -Milliseconds ([int][Math]::Min($IntervalMilliseconds, $remainingMilliseconds))
    }
}

if ($rows.Count -eq 0) {
    throw 'The root process exited before a sample could be collected.'
}

$destination = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputPath)
$directory = Split-Path -Parent $destination
New-Item -ItemType Directory -Force -Path $directory | Out-Null
$rows | Export-Csv -LiteralPath $destination -NoTypeInformation -Encoding utf8
$rows | Measure-Object -Property PrivateWorkingSetMiB, WorkingSetMiB, PrivateBytesMiB -Average -Maximum |
    Select-Object Property, Average, Maximum
Write-Output "Saved $($rows.Count) samples to $destination"
