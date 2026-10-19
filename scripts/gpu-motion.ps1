# A separate instrumented run: engine utilization is not isolated shader/pass duration.
param([int]$FlickPid, [int]$CapturePid, [string]$Output)
$ErrorActionPreference = 'Stop'
$ready = [IO.Path]::ChangeExtension($Output, 'ready')
try {
    $session = (Get-Process -Id $FlickPid).SessionId
    $dwm = @(Get-Process dwm | Where-Object SessionId -eq $session | Select-Object -ExpandProperty Id)
    $pids = @($FlickPid, $CapturePid) + $dwm
    $paths = @((Get-Counter -ListSet '*GPU*' | ForEach-Object Paths) | Where-Object { $_ -like '*Engine*Utilization*' })
    if (!$paths.Count) { throw 'No accessible GPU 3D utilization counters for these processes' }
    $samples = @()
    Set-Content -LiteralPath $ready 'ready'
    Get-Counter -Counter $paths -SampleInterval 1 -MaxSamples 9 | ForEach-Object {
        foreach ($sample in $_.CounterSamples) {
            if ($sample.InstanceName -notlike '*engtype_3D*') { continue }
            $instance=$sample.InstanceName
            if (!@($pids | Where-Object { $instance -match "pid_$($_)_" }).Count) { continue }
            $owner = if ($sample.InstanceName -match "pid_$($FlickPid)_") { 'flick' } elseif ($sample.InstanceName -match "pid_$($CapturePid)_") { 'capture' } else { 'dwm' }
            $samples += [pscustomobject]@{owner=$owner; instance=$sample.InstanceName; type=$sample.CounterType; status=$sample.Status; raw=$sample.RawValue; timestamp100ns=$sample.Timestamp100NS; percent=$sample.CookedValue}
        }
    }
    $samples | Export-Csv -LiteralPath $Output -NoTypeInformation -Encoding UTF8
} catch {
    Set-Content -LiteralPath ($Output + '.error.txt') $_.Exception.Message
    Set-Content -LiteralPath $ready 'unavailable'
}
