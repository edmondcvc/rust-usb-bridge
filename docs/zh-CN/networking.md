# 网络与异地连接

[English](../en/networking.md)
先验证局域网再配置异地网络。主机地址是 Windows 能到达的 Linux 地址；SSH 与 USB TLS 服务使用不同端口。

| Route | Host entered in Windows | SSH | USB TLS |
|---|---|---|---|
| LAN | Linux LAN IP/DNS | 22 | 3241 |
| Tailscale / VPN | Linux private VPN IP/DNS | 22 | 3241 |
| Public relay | Your relay IP/DNS | 12222 (example) | 13241 (example) |

```powershell
Test-NetConnection YOUR_LINUX_ADDRESS -Port 22
Test-NetConnection YOUR_LINUX_ADDRESS -Port 3241
```

```bash
sudo ss -lntp
ip -brief address
```
Tailscale 或其他 VPN 是可选组件，不带入他人的登录状态。两端按各自官方文档加入同一私人网络，先确认互通，再在客户端填写 VPN 地址。

如已有 frp 中转需求，可运行 `scripts/prepare-public-relay.ps1` 生成服务器和客户端配置，参数包括 `-ServerHost`、`-OutputDirectory`、`-PublicSshPort`、`-PublicUsbPort`。使用新建目录；生成的 token 和配置是私密部署文件，不提交 GitHub。按所选 frp 版本的官方文档安装 frps／frpc，只转发 SSH 和 TLS 服务端口，不转发原始 3240。首次及变更后的 SSH 身份仍需核对。

改监听前释放全部设备，再使用 `configure-remote-listener.sh --listen 0.0.0.0:3241 --apply`。同一主机换地址用客户端主机设置保存新地址；换成另一主机则按更换主机流程重新核对身份。
