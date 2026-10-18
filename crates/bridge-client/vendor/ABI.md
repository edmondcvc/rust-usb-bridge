The direct driver interface is fixed to the official **v.0.9.8.1** tag.

- Header: https://github.com/vadimgrn/usbip-win2/blob/v.0.9.8.1/include/usbip/vhci.h
- Constants: https://github.com/vadimgrn/usbip-win2/blob/v.0.9.8.1/include/usbip/consts.h
- Official implementation used to check request sizes and output lengths:
  https://github.com/vadimgrn/usbip-win2/blob/v.0.9.8.1/userspace/libusbip/src/vhci.cpp

Rust compile-time assertions and unit tests check the relevant sizes, offsets,
CTL_CODE values, zero serial override, and malformed driver responses.
`abi-probe.cpp` independently checks the same ABI with Microsoft's C++ compiler
against the unmodified official include directory. This probe does not install
the driver, connect devices, or change USB ownership.

Each attach uses PLUGIN_HARDWARE_ONCE, so the driver does not independently retry
failed/disconnected attachments. USB-Bridge never invokes persistent attachment,
global stop/detach, driver serial replacement, or device reset controls.
