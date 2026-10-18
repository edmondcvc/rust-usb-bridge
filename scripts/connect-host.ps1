[CmdletBinding()]
param([switch]$PipeMode)
$ErrorActionPreference='Stop'
$env:PSModulePath=Join-Path $PSHOME 'Modules'
[Console]::InputEncoding=[Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$OutputEncoding=[Text.UTF8Encoding]::new($false)
function Emit($value) { [Console]::WriteLine(($value | ConvertTo-Json -Depth 8 -Compress)); [Console]::Out.Flush() }
function Fail([string]$kind,[string]$message) { Emit @{event='error';kind=$kind;message=$message}; exit 1 }
if(-not $PipeMode){ throw '请从客户端点击“连接主机”。' }
$password=$null; $request=$null
try {
    $request=[Console]::ReadLine() | ConvertFrom-Json
    $LinuxHost=[string]$request.host; $UserName=[string]$request.username
    $SshPort=[int]$request.ssh_port; $BridgePort=[int]$request.port; $password=[string]$request.password
    $request.password=$null
    if($LinuxHost -notmatch '^[A-Za-z0-9][A-Za-z0-9.:-]*$' -or $UserName -notmatch '^[a-z_][a-z0-9_-]*$' -or $SshPort -lt 1 -or $SshPort -gt 65535 -or $BridgePort -lt 1 -or $BridgePort -gt 65535 -or -not $password -or $password.Contains("`n") -or $password.Contains("`r")) { Fail 'input' '请检查主机地址、用户名、密码和端口。' }
    foreach($tool in @('ssh.exe','ssh-keyscan.exe','ssh-keygen.exe')) { if(-not(Get-Command $tool -ErrorAction SilentlyContinue)){Fail 'missing_ssh' '这台电脑缺少 Windows OpenSSH 客户端。请在 Windows 可选功能中安装 OpenSSH 客户端后重试。'} }
    Emit @{event='progress';message='正在联系 Linux…'}
    $folder=Join-Path $env:LOCALAPPDATA 'RustUsbBridge\owner-authorization'
    if(-not [IO.Path]::GetFullPath($folder).StartsWith('C:\',[StringComparison]::OrdinalIgnoreCase)){Fail 'local' '连接资料需要保存在 C 盘，请检查 Windows 用户目录。'}
    $null=New-Item -ItemType Directory -Path $folder -Force
    $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    & icacls.exe $folder /inheritance:r /grant:r ('*'+$sid+':(OI)(CI)F') '*S-1-5-18:(OI)(CI)F' | Out-Null
    if($LASTEXITCODE -ne 0){Fail 'local' '无法保护本机连接资料，请检查文件夹权限。'}
    $known=Join-Path $folder 'known_hosts'
    $target=if($SshPort -eq 22){$LinuxHost}else{'['+$LinuxHost+']:'+$SshPort}
    function Native([string]$exe,[string[]]$arguments,[string]$inputText='') {
        $info=[Diagnostics.ProcessStartInfo]::new(); $info.FileName=$exe
        $info.Arguments=($arguments | ForEach-Object {'"'+($_ -replace '(\\*)"','$1$1\"' -replace '(\\+)$','$1$1')+'"'}) -join ' '
        $info.UseShellExecute=$false; $info.CreateNoWindow=$true
        $info.RedirectStandardInput=$true; $info.RedirectStandardOutput=$true; $info.RedirectStandardError=$true
        $info.StandardOutputEncoding=[Text.UTF8Encoding]::new($false)
        $p=[Diagnostics.Process]::new(); $p.StartInfo=$info; $null=$p.Start()
        $stdout=$p.StandardOutput.ReadToEndAsync(); $stderr=$p.StandardError.ReadToEndAsync()
        if($inputText){$p.StandardInput.Write($inputText)}; $p.StandardInput.Close()
        $p.WaitForExit()
        $result=@{code=$p.ExitCode;stdout=$stdout.Result;stderr=$stderr.Result}; $p.Dispose(); return $result
    }
    $found=if(Test-Path -LiteralPath $known){ Native 'ssh-keygen.exe' @('-F',$target,'-f',$known) }else{@{code=1}}
    if($found.code -ne 0){
        $scan=Native 'ssh-keyscan.exe' @('-T','8','-p',$SshPort.ToString(),'-t','ed25519',$LinuxHost)
        $keys=@($scan.stdout -split "`n" | Where-Object {$_ -match ' ssh-ed25519 '})
        if($keys.Count -ne 1){Fail 'network' '连不到主机。请检查主机是否开机、地址和登录端口是否正确，以及网络是否打通。'}
        $fingerprint=Native 'ssh-keygen.exe' @('-lf','-') ($keys[0]+"`n")
        if($fingerprint.code -ne 0){Fail 'host_identity' '无法检查主机身份，请重试。'}
        Emit @{event='confirm';host=$LinuxHost;fingerprint=$fingerprint.stdout.Trim()}
        if([Console]::ReadLine() -ne 'yes'){Fail 'cancelled' '已取消连接，可以修改地址后重试。'}
        [IO.File]::AppendAllText($known,$keys[0].Trim()+[Environment]::NewLine,[Text.Encoding]::ASCII)
    }
    $prompt=Join-Path $folder 'PasswordPrompt.exe'
    if(-not(Test-Path -LiteralPath $prompt)){
        Add-Type -OutputAssembly $prompt -OutputType ConsoleApplication -TypeDefinition 'using System; class PasswordPrompt { static void Main() { Console.Write(Environment.GetEnvironmentVariable("BRIDGE_OWNER_PASSWORD")); } }' | Out-Null
    }
    $python=@'
import json,time,pathlib,sys
root=pathlib.Path('/var/lib/rust-usb-bridge')
if not (root/'config.json').is_file() or not (root/'ca.pem').is_file():
    print('BRIDGE_NOT_INSTALLED'); sys.exit(4)
c=json.loads((root/'config.json').read_text())
print(json.dumps(dict(version=1,host='__HOST__',port=__PORT__,server_name=c['server_names'][0],ca_pem=(root/'ca.pem').read_text(),code=c['pair_code'],expires_at=int(time.time())+900)))
'@
    $python=$python.Replace('__HOST__',$LinuxHost).Replace('__PORT__',$BridgePort.ToString())
    if ($request.force_release) {
        $forceJson = $request.force_release | ConvertTo-Json -Depth 6 -Compress
        $forceData = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($forceJson))
        $python = @'
import base64,json,subprocess,sys
request=base64.b64decode('__REQUEST__')
p=subprocess.run(['/opt/rust-usb-bridge/usb-bridge-server','force-release'],input=request,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=25)
if p.returncode:
    print(json.dumps(dict(ok=False,error='Linux 未确认释放。请刷新状态；若服务版本较旧，请先升级 Linux 服务端。')))
else:
    print(p.stdout.decode())
'@
        $python = $python.Replace('__REQUEST__', $forceData)
    }
    $encoded=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($python))
    $command="sudo -k -S -p '' bash -c 'echo $encoded | base64 -d | python3'"
    Emit @{event='progress';message='正在验证 Linux 身份和管理权限…'}
    $env:SSH_ASKPASS=$prompt; $env:SSH_ASKPASS_REQUIRE='force'; $env:DISPLAY='owner-connection'; $env:BRIDGE_OWNER_PASSWORD=$password
    $reply=Native 'ssh.exe' @('-F','NUL','-T','-p',$SshPort.ToString(),'-o','PreferredAuthentications=password','-o','PubkeyAuthentication=no','-o','NumberOfPasswordPrompts=1','-o','StrictHostKeyChecking=yes','-o',('UserKnownHostsFile='+$known),'-o','ConnectTimeout=8','-o','ServerAliveInterval=10','-o','ServerAliveCountMax=2',($UserName+'@'+$LinuxHost),$command) ($password+"`n")
    if($reply.code -ne 0){
        if($reply.stderr -match 'REMOTE HOST IDENTIFICATION HAS CHANGED|Host key verification failed'){Fail 'host_changed' '主机身份变了，已停止连接。请检查地址是否填错，或主机是否重装；确认原因后由维护人员更新已记住的主机身份。'}
        if($reply.stderr -match 'Permission denied'){Fail 'password' '用户名或密码不对。请重新输入主机的用户名和密码。'}
        if($reply.stdout -match 'BRIDGE_NOT_INSTALLED'){Fail 'missing_service' '主机还没有安装 USB 服务，请先完成主机安装。'}
        if($reply.stderr -match 'sudo|incorrect password|not in the sudoers'){Fail 'permission' '已连到主机，但这个用户没有管理权限，或管理密码与登录密码不同。请使用安装主机时设置的管理员用户。'}
        Fail 'network' '连接没有完成。请检查主机、登录端口和网络后重试。'
    }
    if ($request.force_release) {
        $released = $reply.stdout | ConvertFrom-Json
        if ($released.ok -ne $true) { Fail 'release_unconfirmed' 'Linux 未确认释放。设备或使用者可能已变化，请刷新后重试；旧服务端需先升级。' }
        Emit @{event='released'}
        exit 0
    }
    Emit @{event='progress';message='正在获取 USB 列表…'}
    $invite=$reply.stdout | ConvertFrom-Json
    if($invite.version -ne 1 -or -not $invite.ca_pem -or -not $invite.code){Fail 'invalid_reply' '主机没有返回完整的连接信息，请检查 USB 服务是否安装完成。'}
    Emit @{event='complete';invitation=$invite}
} catch { Emit @{event='error';kind='local';message='连接准备没有完成。请检查主机服务或使用新版安装包修复客户端。'}; exit 1 }
finally {
    $password=$null; $request=$null; $reply=$null; $invite=$null
    Remove-Item Env:BRIDGE_OWNER_PASSWORD,Env:SSH_ASKPASS,Env:SSH_ASKPASS_REQUIRE,Env:DISPLAY -ErrorAction SilentlyContinue
}
