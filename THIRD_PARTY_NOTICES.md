# Third-party notices

The Rust USB Bridge application is separate from the kernel drivers. The Windows distribution includes an unchanged upstream installer for **usbip-win2 0.9.8.1 x64**. The Linux distribution uses the operating system's `usbip-core` and `usbip-host` kernel modules; it does not redistribute a Linux kernel or replace its drivers.

- Upstream project: <https://github.com/vadimgrn/usbip-win2>
- Pinned release: <https://github.com/vadimgrn/usbip-win2/releases/tag/v.0.9.8.1>
- PnP driver version supplied by this release: `12.1.30.403` (distinct from the product/ABI release `0.9.8.1`).
- Original installer: `USBip-0.9.8.1-x64.exe`
- SHA-256: `38cad6d4432b52d5bb9409d9ad03b72fdffc4ada4cd3a48fbeca1a2752a8518a`
- Upstream license source: <https://github.com/vadimgrn/usbip-win2/blob/v.0.9.8.1/LICENSE.txt>

The upstream installer includes its own license, signed driver package, and Microsoft runtime redistributable. Those files are retained as supplied by their authors. Rust crate dependencies and their versions are recorded in `Cargo.lock`; unchanged dependency and bundled font license/notice texts are provided in `THIRD_PARTY_LICENSES`, with version, SPDX declaration, source URL and text-hash metadata in its `INDEX.json`. The application MIT license and these dependency licenses apply to their respective parts; they are not replaced by the following upstream notice.

## usbip-win2 license

```text
BSD 2-Clause License

Copyright (c) 2021-2026, Vadym Hrynchyshyn <vadimgrn@gmail.com>

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.
THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```
