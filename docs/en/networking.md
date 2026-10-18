# Networking and remote access

[中文](../zh-CN/networking.md)
Verify the LAN route first. The host address must be reachable from Windows; SSH and the USB TLS service use separate ports.

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
Tailscale or another VPN is optional; no existing login identity is bundled. Join both machines to your own private network using the provider's official instructions, verify reachability, then enter the VPN address.

For an existing frp deployment, `scripts/prepare-public-relay.ps1` generates server/client configuration using `-ServerHost`, `-OutputDirectory`, `-PublicSshPort` and `-PublicUsbPort`. Use a new directory. Generated tokens/configuration are private deployment files, never GitHub content. Install frps/frpc according to the selected upstream version and forward SSH plus TLS, not raw port 3240. Verify SSH identity on first use and investigate changes.

Release all devices before applying listener changes with `configure-remote-listener.sh --listen 0.0.0.0:3241 --apply`. Use host settings when changing the address of the same host; replacing the host requires the explicit replace-host identity flow.
