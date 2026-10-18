# Windows Rust USB 客户端与后台服务

双击 `bridge-client.exe` 打开中文界面。界面通过受本机访问控制限制的命名管道与
`UsbBridgeRust` 服务通信；关闭窗口不结束已连接设备。服务使用 LocalService
账户，USB/IP 驱动保持官方 0.9.8.1 签名文件完整。
客户端在发送配对码或控制请求前，将管道的真实服务端进程 ID 与 Windows 服务管理器
记录的运行中 `UsbBridgeRust` 进程核对。未安装正式服务或身份不一致时直接拒绝连接。
交互用户只获得管道数据访问权，不获得创建额外管道实例的权限。

安装程序调用以下命令，然后由 Windows 服务管理器启动服务：

```text
bridge-client.exe --install-service --config C:\ProgramData\RustUsbBridge\client.json
bridge-client.exe --service --config C:\ProgramData\RustUsbBridge\client.json
bridge-client.exe --uninstall-service
```

默认配置可以只包含 host、port、server_name、display_name、proxy_port、client_id、
auto_connect。其余字段使用默认值。初始 proxy_port 为 0，后台第一次启动后分配
一个本机回环端口并保存；以后严格使用这个端口，冲突时报错，不改变设备身份。

配对界面需要 Linux 地址、证书名称、Linux 导出的可信 CA PEM 文件及一次性配对码。
客户端校验证书链和证书名称，没有“忽略证书错误”模式。配对成功后 CA、客户端证书
和私钥保存在 config 目录下 `credentials/<client_id>/`，该目录仅服务账户、管理员、
SYSTEM 可以读取。配置的原子替换是配对提交点，重新配对失败不覆盖此前的证书。

自动连接默认关闭；用户选中后只在后台启动时尝试一次。记录 Linux server_id、物理
路径、VID/PID 和真实序列号；不同设备插到相同端口时不会自动连接。CH341 EPP
`1a86:5512` 始终手动连接。连接使用 PLUGIN_HARDWARE_ONCE，断线后不重放请求，
不让驱动自行恢复连接。设备本来的序列号不会被覆盖。

诊断命令（Windows Release 版本采用 GUI 子系统，建议使用 `--output` 保存结果）：

```text
bridge-client.exe --diagnostics --output C:\USB诊断.json
bridge-client.exe --status --output C:\USB后台状态.json
bridge-client.exe --list --output C:\USB设备列表.json
bridge-client.exe --connect 1-2 --output C:\USB连接结果.json
bridge-client.exe --release 1-2 --output C:\USB释放结果.json
bridge-client.exe --export-logs C:\USB日志.txt --output C:\USB日志导出结果.json
```

`--connect` 和 `--release` 是实际设备操作，不能用作无人值守硬件测试。
`--diagnostics` 只查询驱动接口、版本和已导入列表，不连接或释放设备。
Windows 上显式使用用户自有目录进行 `--direct` 软件诊断时，凭据目录还授权给
实际创建配置的账户 SID，避免 UAC 过滤管理员组后使创建者无法读写。正常 SCM
运行账户仍为 LocalService，不会将生产服务私钥授权给交互用户。

跨平台网络集成测试可显式绕过 Windows 后台管道：

```text
bridge-client --direct --config client.json --pair --host 127.0.0.1 --port 3241 --server-name usb-bridge --ca ca.pem --code 一次性码 --name 测试电脑
bridge-client --direct --config client.json --list
bridge-client --direct --config client.json --status
```

`--background --config PATH` 显式启动软件诊断后台，使用独立的
`RustUsbBridge.Diagnostic.<进程ID>` 管道及用户自己的配置目录。连接此后台必须显式指定
`--diagnostic-ipc <进程ID>`，客户端仍核验操作系统返回的管道服务端 ID，普通 GUI/CLI
始终只连接经 SCM 核验的生产后台。诊断 GUI 的窗口标题显示“显式诊断模式”。
不要把生产服务配置传给诊断后台；正常用户界面不会自动启动临时后台或复制服务私钥。
例如先在自己的测试目录启动 `--background` 并取得其进程 ID，然后执行
`bridge-client.exe --diagnostic-ipc 12345 --status --output C:\USB诊断后台状态.json`。
`--proxy --config PATH --proxy-port 0 --allow-busid 1-2` 只启动显式授权端口的本机代理，
用于模拟 USB/IP 流测试；此命令不会调用 Windows 驱动。

排查管理请求超时时，可仅在该 CLI 子进程设置 `RUST_LOG=bridge_client::network=debug`，
并保存标准错误输出。日志只记录操作类型、TCP/TLS/写入/flush/响应读取阶段及耗时，
不记录配对码、客户端名称、证书、私钥或 JSON 内容。超时错误本身也报告最后阶段；
TCP/TLS 建立仍限 10 秒。目标 Linux 启动盘已实测单次 fsync 等待约 22.5 秒，
配置保存还需多次同步，因此管理请求总期限为 90 秒；Windows IPC 为 120 秒。
共享修改直接使用成功响应中的新列表，不追加第二次查询。配对已提交后的列表刷新
单独限 15 秒，刷新失败只显示网络状态，不把已经完成的配对显示为失败。
超时后应先刷新状态确认结果，没有自动重试。不要设置全局环境变量。
配对或共享保存进行中，停服/卸载会等待当前操作完成，最多等待 180 秒；不强杀进程。

运行时每次 IOCTL 之前都通过真实 PnP 接口核实驱动版本必须为 0.9.8.1，拒绝不匹配
的外部驱动。`vendor/abi-probe.cpp` 与 Rust 静态断言/测试分别核实 ABI。

软件测试覆盖双设备 mTLS 代理透明传输、单设备断开隔离、固定端口、一次会话授权、
帧长度边界、驱动响应格式、真实设备身份、无效 CA 拒绝、普通用户管道读写、额外实例
创建被拒及服务端 PID 不匹配时不发送任何请求。Windows 管道 ACL 测试必须在普通
交互用户、未提升管理员权限的进程中运行。模拟传输通过不代表摄像头、
声卡、雕刻机等物理设备已经通过验收；实际安装、原功能驱动绑定及加工测试必须另行记录。
