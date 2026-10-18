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
