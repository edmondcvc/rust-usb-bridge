// Compile with MSVC x64 against the unmodified official v.0.9.8.1
// include directory. This independently checks the Rust FFI layout.
#include <cstdio>
#include <usbip/vhci.h>
int main() {
    using namespace usbip::vhci;
    ioctl::plugin_hardware p{};
    imported_device d{};
    auto offset = [](const void* object, const void* field) { return (size_t)((const char*)field - (const char*)object); };
    std::printf("location=%zu properties=%zu imported=%zu plugin=%zu stop=%zu get=%zu\n",
        sizeof(imported_device_location), sizeof(imported_device_properties), sizeof(imported_device), sizeof(p),
        sizeof(ioctl::stop_attach_attempts), sizeof(ioctl::get_imported_devices));
    std::printf("port=%zu busid=%zu service=%zu host=%zu serial=%zu wsk=%zu imported_properties=%zu\n",
        offset(&p,&p.port),offset(&p,p.busid),offset(&p,p.service),offset(&p,p.host),offset(&p,p.serial),
        offset(&p,&p.wsk_events),offset(&d,&d.devid));
    return sizeof(p)==1124 && sizeof(d)==1132 && offset(&p,p.serial)==1104 ? 0 : 1;
}
