[CmdletBinding()]
param([string]$LinuxHost='', [string]$UserName='operator', [int]$SshPort=22, [int]$BridgePort=3241)
$ErrorActionPreference='Stop'
# The GUI starts Windows PowerShell 5.1 from a parent that may use PS7 modules.
$env:PSModulePath=Join-Path $PSHOME 'Modules'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName Microsoft.VisualBasic
$password=$null
try {
    if (-not $LinuxHost) {
        $form=[Windows.Forms.Form]::new(); $form.Text='用 Linux 密码授权电脑'; $form.Width=520; $form.Height=300; $form.StartPosition='CenterScreen'
        $addressLabel=[Windows.Forms.Label]::new(); $addressLabel.Text='Linux 地址（局域网 / Tailscale / VPN / 公网中转）'; $addressLabel.SetBounds(20,20,470,24)
        $address=[Windows.Forms.TextBox]::new(); $address.SetBounds(20,48,460,28)
        $sshLabel=[Windows.Forms.Label]::new(); $sshLabel.Text='登录端口：通常 22；公网中转按配置填写'; $sshLabel.SetBounds(20,88,460,24)
        $sshNumber=[Windows.Forms.NumericUpDown]::new(); $sshNumber.Minimum=1; $sshNumber.Maximum=65535; $sshNumber.Value=$SshPort; $sshNumber.SetBounds(20,114,120,26)
        $usbLabel=[Windows.Forms.Label]::new(); $usbLabel.Text='USB 服务端口：通常 3241；公网中转按配置填写'; $usbLabel.SetBounds(20,151,460,24)
        $usbNumber=[Windows.Forms.NumericUpDown]::new(); $usbNumber.Minimum=1; $usbNumber.Maximum=65535; $usbNumber.Value=$BridgePort; $usbNumber.SetBounds(20,178,120,26)
        $next=[Windows.Forms.Button]::new(); $next.Text='下一步，输入 Linux 密码'; $next.SetBounds(240,218,240,30); $next.DialogResult='OK'
        $form.Controls.AddRange(@($addressLabel,$address,$sshLabel,$sshNumber,$usbLabel,$usbNumber,$next)); $form.AcceptButton=$next
        if($form.ShowDialog() -ne 'OK'){ $form.Dispose(); return }
        $LinuxHost=$address.Text.Trim(); $SshPort=[int]$sshNumber.Value; $BridgePort=[int]$usbNumber.Value; $form.Dispose()
        if(-not $LinuxHost){throw '请填写 Linux 主机地址。'}
    }
    if ($LinuxHost -notmatch '^[A-Za-z0-9][A-Za-z0-9.:-]*$' -or $UserName -notmatch '^[a-z_][a-z0-9_-]*$' -or $SshPort -lt 1 -or $SshPort -gt 65535 -or $BridgePort -lt 1 -or $BridgePort -gt 65535) { throw '主机地址、用户名或端口无效。' }
    $folder=Join-Path $env:LOCALAPPDATA 'RustUsbBridge\owner-authorization'
    if (-not [IO.Path]::GetFullPath($folder).StartsWith('C:\',[StringComparison]::OrdinalIgnoreCase)) { throw '授权文件必须保存在 C:。' }
    $null=New-Item -ItemType Directory -Path $folder -Force
    & icacls.exe $folder /inheritance:r /grant:r ($env:USERNAME+':(OI)(CI)F') 'SYSTEM:(OI)(CI)F' | Out-Null
    if($LASTEXITCODE -ne 0) { throw '无法保护本地授权目录。' }
    $known=Join-Path $folder 'known_hosts'
    $target=if($SshPort -eq 22){$LinuxHost}else{'['+$LinuxHost+']:'+$SshPort}
    $found=if(Test-Path -LiteralPath $known){ & ssh-keygen.exe -F $target -f $known 2>$null }else{$null}
    if(-not $found) {
        $scan=@(& ssh-keyscan.exe -T 8 -p $SshPort -t ed25519 $LinuxHost 2>$null | Where-Object {$_ -match ' ssh-ed25519 '})
        if($scan.Count -ne 1) { throw '无法连接 Linux 的 SSH 服务，请检查地址、网络和端口。' }
        $fingerprint=($scan[0] | & ssh-keygen.exe -lf - 2>$null) -join ''
        $answer=[Windows.Forms.MessageBox]::Show("首次连接主机 $LinuxHost。请核对 Linux 屏幕或可信 FinalShell 连接中的主机指纹：`n$fingerprint`n确认这是自己的主机后继续。",'确认自己的 Linux 主机','YesNo','Question')
        if($answer -ne 'Yes') { return }
        $scan[0] | Add-Content -LiteralPath $known -Encoding ASCII
    }
    $credential=Get-Credential -UserName $UserName -Message '输入这台 Linux 的密码，授权当前电脑。密码只用于本次连接，不保存。'
    if(-not $credential){return}
    $password=$credential.GetNetworkCredential().Password
    $prompt=Join-Path $folder 'PasswordPrompt.exe'
    if(-not(Test-Path -LiteralPath $prompt)) {
        Add-Type -OutputAssembly $prompt -OutputType ConsoleApplication -TypeDefinition 'using System; class PasswordPrompt { static void Main() { Console.Write(Environment.GetEnvironmentVariable("BRIDGE_OWNER_PASSWORD")); } }'
    }
    $python=@'
import json,time,pathlib
root=pathlib.Path('/var/lib/rust-usb-bridge')
c=json.loads((root/'config.json').read_text())
print(json.dumps(dict(version=1,host='__HOST__',port=__PORT__,server_name=c['server_names'][0],ca_pem=(root/'ca.pem').read_text(),code=c['pair_code'],expires_at=int(time.time())+900)))
'@
    $python=$python.Replace('__HOST__',$LinuxHost).Replace('__PORT__',$BridgePort.ToString())
    $encoded=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($python))
    $command="sudo -S -p '' bash -c 'echo $encoded | base64 -d | python3'"
    # No password in process arguments or files. Both SSH and sudo use this process's memory.
    $env:SSH_ASKPASS=$prompt; $env:SSH_ASKPASS_REQUIRE='force'; $env:DISPLAY='owner-authorization'; $env:BRIDGE_OWNER_PASSWORD=$password
    $reply=($password+"`n") | & ssh.exe -F NUL -T -p $SshPort -o PreferredAuthentications=password -o PubkeyAuthentication=no -o NumberOfPasswordPrompts=1 -o StrictHostKeyChecking=yes -o ('UserKnownHostsFile='+$known) -o ConnectTimeout=8 ($UserName+'@'+$LinuxHost) $command 2>$null
    if($LASTEXITCODE -ne 0){throw '授权信息获取失败。请检查 Linux 密码、sudo 权限，以及 USB 服务是否已安装。'}
    $invite=($reply -join "`n") | ConvertFrom-Json
    if($invite.version -ne 1 -or -not $invite.ca_pem -or -not $invite.code){throw '主机返回的授权信息不完整。'}
    $output=Join-Path $folder 'owner-invitation.json'
    [IO.File]::WriteAllText($output,($invite | ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
    [Windows.Forms.MessageBox]::Show('授权信息已准备好。返回客户端，点击“导入刚生成的授权”，再点击“授权并查看设备”。无需另一台已授权电脑。','授权准备完成') | Out-Null
} catch {
    [Windows.Forms.MessageBox]::Show($_.Exception.Message,'授权未完成') | Out-Null
    exit 1
} finally {
    $password=$null; $credential=$null
    Remove-Item Env:BRIDGE_OWNER_PASSWORD,Env:SSH_ASKPASS,Env:SSH_ASKPASS_REQUIRE,Env:DISPLAY -ErrorAction SilentlyContinue
}
