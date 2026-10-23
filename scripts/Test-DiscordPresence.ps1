#Requires -Version 7.0
# Uses Discord's local IPC protocol:
# https://docs.discord.com/developers/topics/rpc
[CmdletBinding()]
param(
    [ValidatePattern('^\d+$')]
    [string]$ApplicationId = '1562983781768626278',
    [string]$AssetKey = 'ferric',
    [string]$DisplayName = 'YouTube Music (Ferric)',
    [ValidateRange(5, 600)]
    [int]$Seconds = 120
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Read-ExactBytes {
    param($Pipe, [int]$Count, $Token)
    $buffer = [byte[]]::new($Count)
    $offset = 0
    while ($offset -lt $Count) {
        $read = $Pipe.ReadAsync($buffer, $offset, $Count - $offset, $Token).GetAwaiter().GetResult()
        if ($read -eq 0) { throw 'Discord disconnected.' }
        $offset += $read
    }
    return ,$buffer
}

function Read-DiscordFrame {
    param($Pipe, [int]$TimeoutMilliseconds = 5000)
    $cancellation = [System.Threading.CancellationTokenSource]::new($TimeoutMilliseconds)
    try {
        $header = Read-ExactBytes $Pipe 8 $cancellation.Token
        $opcode = [BitConverter]::ToUInt32($header, 0)
        $length = [BitConverter]::ToUInt32($header, 4)
        if ($length -gt 65536) { throw 'Discord returned an oversized frame.' }
        $bytes = Read-ExactBytes $Pipe ([int]$length) $cancellation.Token
        $data = [Text.Encoding]::UTF8.GetString($bytes) | ConvertFrom-Json -AsHashtable
        return @{ Opcode = $opcode; Bytes = $bytes; Data = $data }
    } catch [System.OperationCanceledException] {
        throw [System.TimeoutException]::new('Timed out waiting for Discord.')
    } finally {
        $cancellation.Dispose()
    }
}

function Send-DiscordFrame {
    param($Pipe, [uint32]$Opcode, [byte[]]$Bytes)
    $frame = [byte[]]::new(8 + $Bytes.Length)
    [BitConverter]::GetBytes($Opcode).CopyTo($frame, 0)
    [BitConverter]::GetBytes([uint32]$Bytes.Length).CopyTo($frame, 4)
    $Bytes.CopyTo($frame, 8)
    $cancellation = [System.Threading.CancellationTokenSource]::new(5000)
    try {
        $Pipe.WriteAsync($frame, 0, $frame.Length, $cancellation.Token).GetAwaiter().GetResult()
    } finally {
        $cancellation.Dispose()
    }
}

function Send-DiscordJson {
    param($Pipe, [uint32]$Opcode, $Payload)
    $bytes = [Text.Encoding]::UTF8.GetBytes(($Payload | ConvertTo-Json -Depth 10 -Compress))
    Send-DiscordFrame $Pipe $Opcode $bytes
}

function Receive-DiscordResponse {
    param($Pipe, [string]$Nonce = '')
    $deadline = [DateTimeOffset]::UtcNow.AddSeconds(5)
    while ([DateTimeOffset]::UtcNow -lt $deadline) {
        $remaining = [Math]::Max(1, [int]($deadline - [DateTimeOffset]::UtcNow).TotalMilliseconds)
        $frame = Read-DiscordFrame $Pipe $remaining
        if ($frame.Opcode -eq 3) {
            Send-DiscordFrame $Pipe 4 $frame.Bytes
            continue
        }
        if ($frame.Opcode -ne 1) { throw 'Discord closed or rejected the IPC session.' }
        $data = $frame.Data
        if ($data['evt'] -eq 'ERROR') {
            throw "Discord error $($data['data']['code']): $($data['data']['message'])"
        }
        if (($Nonce -eq '' -and $data['evt'] -eq 'READY') -or
            ($Nonce -ne '' -and $data['nonce'] -eq $Nonce -and $data['cmd'] -eq 'SET_ACTIVITY')) {
            return $data
        }
    }
    throw 'Discord did not acknowledge the request.'
}

function Set-TestActivity {
    param($Pipe, $Activity)
    $nonce = [Guid]::NewGuid().ToString()
    Send-DiscordJson $Pipe 1 @{
        cmd = 'SET_ACTIVITY'
        args = @{ pid = $PID; activity = $Activity }
        nonce = $nonce
    }
    return Receive-DiscordResponse $Pipe $nonce
}

$pipe = $null
$published = $false
$accessDenied = $false
try {
    for ($index = 0; $index -lt 10; $index++) {
        $candidate = [System.IO.Pipes.NamedPipeClientStream]::new(
            '.', "discord-ipc-$index", [System.IO.Pipes.PipeDirection]::InOut,
            [System.IO.Pipes.PipeOptions]::Asynchronous)
        try {
            $candidate.Connect(200)
            $pipe = $candidate
            Write-Host "Connected to discord-ipc-$index."
            break
        } catch {
            Write-Verbose "discord-ipc-${index}: $($_.Exception.Message)"
            if ($_.Exception.InnerException -is [System.UnauthorizedAccessException]) {
                $accessDenied = $true
            }
            $candidate.Dispose()
        }
    }
    if ($null -eq $pipe) {
        if ($accessDenied) {
            throw 'Access to Discord IPC was denied. Run outside a restricted sandbox, under the same Windows account as Discord.'
        }
        throw 'Open the Discord desktop client and sign in, then run this script again.'
    }

    Send-DiscordJson $pipe 0 @{ v = 1; client_id = $ApplicationId }
    $null = Receive-DiscordResponse $pipe
    Write-Host "Discord accepted the handshake for application $ApplicationId."

    $started = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    $published = $true # Attempt cleanup even if publication succeeds but its acknowledgement is lost.
    $response = Set-TestActivity $pipe @{
        type = 2
        name = $DisplayName
        status_display_type = 2
        details = 'Ferric Rich Presence test'
        state = 'Setup verification (sample track)'
        timestamps = @{ start = $started; end = $started + $Seconds }
        assets = @{ large_image = $AssetKey; large_text = 'Ferric - YouTube Music for Windows' }
        buttons = @(@{ label = 'Open YouTube Music'; url = 'https://music.youtube.com/' })
        instance = $false
    }
    Write-Host "Discord acknowledged SET_ACTIVITY. Returned activity type: $($response['data']['type'])."
    Write-Host "Display name: $($response['data']['name']); member-list status mode: $($response['data']['status_display_type'])."
    Write-Host "Look at your Discord profile: sample Listening card, '$AssetKey' artwork, progress, and a link."
    Write-Host "The test will clear itself after $Seconds seconds. Ctrl+C also closes the connection."

    $deadline = [DateTimeOffset]::UtcNow.AddSeconds($Seconds)
    while ([DateTimeOffset]::UtcNow -lt $deadline) {
        $remaining = [Math]::Max(1, [int]($deadline - [DateTimeOffset]::UtcNow).TotalMilliseconds)
        try { $frame = Read-DiscordFrame $pipe $remaining }
        catch [System.TimeoutException] { break }
        if ($frame.Opcode -eq 3) { Send-DiscordFrame $pipe 4 $frame.Bytes }
        elseif ($frame.Opcode -eq 2) { throw 'Discord closed the test connection.' }
    }
} finally {
    if ($null -ne $pipe) {
        try {
            if ($published -and $pipe.IsConnected) {
                $null = Set-TestActivity $pipe $null
                Write-Host 'Discord acknowledged clearing the test presence.'
            }
        } catch {
            Write-Warning "Could not confirm clearing the presence: $($_.Exception.Message)"
        } finally {
            $pipe.Dispose()
        }
    }
}
