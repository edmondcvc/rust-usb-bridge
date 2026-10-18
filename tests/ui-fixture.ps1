# Explicit diagnostic-only IPC fixture. Never opens the production pipe or USB.
param([Parameter(Mandatory=$true)][string]$PidFile,
    [ValidateSet('readonly','success','ready','prepare_failure','owner_race','driver_failure')][string]$Scenario='readonly',
    [string]$RecordFile='')
$ErrorActionPreference = 'Stop'
if (-not [IO.Path]::GetFullPath($PidFile).StartsWith('C:\', [StringComparison]::OrdinalIgnoreCase)) { throw 'C: output required' }
$PID | Set-Content -LiteralPath $PidFile
$config = @{host='127.0.0.1';port=3241;server_name='fixture';display_name='演示：本机';proxy_port=0;client_id='fixture';credential_generation=$null;auto_connect=@();auto_connect_identities=@{}}
function New-FixtureDevice($id,$name,$shared,$owner) {
    @{busid=$id;name=$name;vendor=4660;product=22136;serial=$null;path=('/fixture/' + $id);speed=3;shared=$shared;owner=$owner;protected_reason=$null;present=$true}
}
$devices = @(
    (New-FixtureDevice '1-1' '模拟设备：车间摄像头' $true '车间电脑-A'),
    (New-FixtureDevice '1-2' '模拟设备：本机 U 盘' $true '演示：本机'),
    (New-FixtureDevice '1-3' '模拟设备：空闲 USB' $true $null),
    (New-FixtureDevice '1-4' '模拟设备：尚未共享' $false $null)
)
$sessions = @(@{busid='1-2';port=1;state='connected';error=$null})
if ($Scenario -ne 'readonly') {
    $devices = @((New-FixtureDevice '1-4' '模拟设备：点击一次连接 USB' ($Scenario -eq 'ready') $null))
    $sessions = @()
}
if ($RecordFile -and -not [IO.Path]::GetFullPath($RecordFile).StartsWith('C:\', [StringComparison]::OrdinalIgnoreCase)) { throw 'C: recording required' }
while ($true) {
    $pipe = [IO.Pipes.NamedPipeServerStream]::new(('RustUsbBridge.Diagnostic.' + $PID), [IO.Pipes.PipeDirection]::InOut, 1, [IO.Pipes.PipeTransmissionMode]::Byte)
    try {
        $pipe.WaitForConnection()
        $head = [byte[]]::new(4)
        $read = 0
        while ($read -lt 4) { $n = $pipe.Read($head,$read,4-$read); if ($n -eq 0) { throw 'EOF' }; $read += $n }
        [Array]::Reverse($head)
        $length = [BitConverter]::ToUInt32($head,0)
        if ($length -gt 1048576) { throw 'Too large' }
        $body = [byte[]]::new($length)
        $read = 0
        while ($read -lt $length) { $n = $pipe.Read($body,$read,$length-$read); if ($n -eq 0) { throw 'EOF' }; $read += $n }
        $request = [Text.Encoding]::UTF8.GetString($body) | ConvertFrom-Json
        $ok = $request.op -eq 'snapshot'
        $errorText = '模拟界面不执行设备操作'
        if ($Scenario -ne 'readonly' -and $request.busid -eq '1-4') {
            if ($RecordFile -and $request.op -ne 'snapshot') {
                @{op=$request.op;busid=$request.busid;shared=$request.shared} | ConvertTo-Json -Compress | Add-Content -LiteralPath $RecordFile -Encoding utf8
            }
            if ($request.op -eq 'set_shared' -and $request.shared -eq $true) {
                $ok = $Scenario -ne 'prepare_failure'
                if ($ok) { $devices[0].shared = $true } else { $errorText = '模拟设备准备失败' }
                if ($Scenario -eq 'owner_race') { $devices[0].owner = '模拟：其他电脑抢先连接' }
            }
            if ($request.op -eq 'connect') {
                $ok = $devices[0].shared -and $null -eq $devices[0].owner -and $Scenario -ne 'driver_failure'
                if ($ok) {
                    $devices[0].owner = $config.display_name
                    $sessions = @(@{busid='1-4';port=1;state='connected';error=$null})
                } else {
                    $errorText = '模拟 Windows 连接失败'
                    $sessions = @(@{busid='1-4';port=$null;state='failed';error=$errorText})
                }
            }
        }
        $invitation = $null
        if ($request.op -eq 'create_invitation') {
            $ok = $true
            $invitation = @{version=1;host='127.0.0.1';port=3241;server_name='fixture';ca_pem='SIMULATED-NOT-A-REAL-CERTIFICATE';code='SIMULATED-INVITATION';expires_at=([DateTimeOffset]::UtcNow.ToUnixTimeSeconds()+900)} | ConvertTo-Json -Compress
        }
        if ($request.op -eq 'release_all') {
            $ok = $true
            foreach ($device in $devices) { if ($device.owner -eq $config.display_name) { $device.owner = $null } }
            $sessions = @(); $config.auto_connect = @(); $config.auto_connect_identities = @{}
            if ($RecordFile) { '{"op":"release_all"}' | Add-Content -LiteralPath $RecordFile -Encoding utf8 }
        }
        $reply = @{invitation=$invitation;ok=$ok;error=$(if ($ok) { $null } else { $errorText });log=$null;snapshot=@{
            config=$config;paired=$true;server_id='UI fixture - no hardware';devices=$devices;
            sessions=$sessions;driver_error=$null;network_error=$null;proxy_port=0;
            devices_checked_at_ms=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        }}
        $bytes = [Text.Encoding]::UTF8.GetBytes(($reply | ConvertTo-Json -Depth 12 -Compress))
        $head = [BitConverter]::GetBytes([uint32]$bytes.Length); [Array]::Reverse($head)
        $pipe.Write($head,0,4); $pipe.Write($bytes,0,$bytes.Length); $pipe.Flush()
    } catch { Write-Warning $_.Exception.Message }
    finally { $pipe.Dispose() }
}
