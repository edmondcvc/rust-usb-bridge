# Platform support and acceptance

[中文](../zh-CN/support.md)
This is a prerelease. Targets below are not blanket production-certification claims. ARM/Pi physical hardware is unavailable, so stable support waits for hardware acceptance.

| Target | OS baseline | Status |
|---|---|---|
| Linux x86_64 | Ubuntu Server 24.04 LTS | Build / automated checks; public-version physical acceptance pending |
| Generic UEFI ARM64 | Ubuntu Server 24.04 LTS ARM64 | Build target; physical boot and USB pending |
| Raspberry Pi 4 | Ubuntu Server 24.04 LTS raspi 64-bit | Image recipe; board acceptance pending |
| Raspberry Pi 5 | Ubuntu Server 24.04 LTS raspi 64-bit | Image recipe; board acceptance pending |
| Windows x64 | Windows 10 1903+ / Windows 11 | Installer target; public-version clean-machine acceptance pending |
| Debian / other systemd distributions | Matching USB/IP kernel required | Compatibility path; not formally qualified |
| VM | Explicit physical USB passthrough required | Boot/mock tests do not prove physical forwarding |
| Containers / WSL | Host kernel, sysfs, privileges required | Not a supported production installation route |

Stable gate: clean installation, pairing, real USB data transfer/release, network drop and reboot recovery per platform; Pi 4/5 are separate records. Two Windows machines must use different devices concurrently, same-device ownership must be exclusive, and releasing one computer must not affect the other. Installer checks, mock USB, ARM runners and VM tests are not substitutes.

Every hardware record must identify version, source commit, artifact hash, OS/kernel, board model, procedure and observed result. Publish `v0.2.0` only after all target hardware passes. Compatibility of each device type, especially audio/video and machinery, requires corresponding real-device testing.

## Published RC installation validation — 2026-10-18

Tested the published `v0.2.0-rc.1` assets from source commit `5e183c74e9b834655182a71c55a1173c79295222` in an isolated x86_64 QEMU VM, Ubuntu 24.04, kernel `6.8.0-147-generic`. The Linux package SHA256 was `f647c757e26d5ebcaa1b1d731280f7dc953808fe5cbcab2b7a020440bc758684`; the online installer SHA256 was `4b0cff51ee39b1ca701976d181afd88159ab82e4a8878b2ac7992158370d23e0`.

| Procedure | Observed result |
|---|---|
| Online install without USB/IP modules | Refused before creating the application installation directory |
| Install matching kernel modules, retry published online installer | Service active, version `0.2.0-rc.1` |
| Verify package checksums, reinstall offline | Service active; configuration and CA hashes unchanged |
| Roll back using a complete backup | Service active; configuration and CA hashes unchanged |
| Request rollback using an incomplete backup | Refused before stopping the running service |
| Uninstall, then reinstall offline | Own binary/service removed; retained configuration and CA reused; service active |

Both rollback binaries were the same RC version: this validates the command and identity retention, not an older-version compatibility matrix. No Windows client was paired in this VM and no physical USB was attached. These results do not establish actual paired-client retention, hardware USB forwarding, or boot-image acceptance.
