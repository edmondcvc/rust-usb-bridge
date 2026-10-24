//! Windows GUI lifetime. CLI commands and the background service are unaffected.
use anyhow::{bail, Result};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND},
    System::Threading::CreateMutexW,
    UI::WindowsAndMessaging::*,
};

const ACTIVATE: u32 = WM_APP + 73;
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn property(demo: bool) -> Vec<u16> {
    #[cfg(test)]
    return wide(if demo {
        "RustUsbBridge.Tests.Demo.v1"
    } else {
        "RustUsbBridge.Tests.v1"
    });
    #[cfg(not(test))]
    wide(if demo {
        "RustUsbBridge.Gui.Demo.v1"
    } else {
        "RustUsbBridge.Gui.v1"
    })
}

pub struct Instance(HANDLE);
impl Instance {
    pub fn acquire(demo: bool) -> Result<Option<Self>> {
        let marker = property(demo);
        let name = wide(&format!(
            "Local\\{}",
            String::from_utf16_lossy(&marker[..marker.len() - 1])
        ));
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let guard = Self(handle);
        if !exists {
            return Ok(Some(guard));
        }
        // The first process may still be creating its HWND. Keep the mutex handle
        // alive during discovery; never launch a competing window on timeout.
        let marker = property(demo);
        for _ in 0..100 {
            let mut search = Search {
                marker: marker.as_ptr(),
                hwnd: std::ptr::null_mut(),
            };
            unsafe {
                EnumWindows(Some(find_window), &mut search as *mut Search as isize);
            }
            if !search.hwnd.is_null() {
                unsafe {
                    let mut pid = 0;
                    GetWindowThreadProcessId(search.hwnd, &mut pid);
                    AllowSetForegroundWindow(pid);
                    if PostMessageW(search.hwnd, ACTIVATE, 0, 0) == 0 {
                        return Err(std::io::Error::last_os_error().into());
                    }
                }
                return Ok(None);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        bail!("USB 连接中心已经在启动或运行，请稍后从任务栏或右下角托盘打开。")
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Search {
    marker: *const u16,
    hwnd: HWND,
}
unsafe extern "system" fn find_window(hwnd: HWND, data: isize) -> i32 {
    let search = &mut *(data as *mut Search);
    if !GetPropW(hwnd, search.marker).is_null() {
        search.hwnd = hwnd;
        0
    } else {
        1
    }
}

pub struct WindowHook {
    hwnd: HWND,
    previous: isize,
    marker: Vec<u16>,
}
impl WindowHook {
    pub fn install(hwnd: isize, demo: bool, tray_available: bool) -> Result<Self> {
        let hwnd = hwnd as HWND;
        let marker = property(demo);
        unsafe {
            let previous = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
            if previous == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if SetPropW(
                hwnd,
                wide("RustUsbBridge.PreviousProc").as_ptr(),
                previous as HANDLE,
            ) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            if SetWindowLongPtrW(hwnd, GWLP_WNDPROC, window_proc as *const () as isize) == 0 {
                RemovePropW(hwnd, wide("RustUsbBridge.PreviousProc").as_ptr());
                return Err(std::io::Error::last_os_error().into());
            }
            let hook = Self {
                hwnd,
                previous,
                marker,
            };
            if tray_available
                && SetPropW(
                    hwnd,
                    wide("RustUsbBridge.CloseToDock").as_ptr(),
                    1usize as HANDLE,
                ) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            // Publish only after the handler is ready for activation messages.
            if SetPropW(hwnd, hook.marker.as_ptr(), 1usize as HANDLE) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(hook)
        }
    }
    pub fn allow_exit(&self) {
        unsafe {
            RemovePropW(self.hwnd, wide("RustUsbBridge.CloseToDock").as_ptr());
        }
    }
}
impl Drop for WindowHook {
    fn drop(&mut self) {
        unsafe {
            RemovePropW(self.hwnd, self.marker.as_ptr());
            RemovePropW(self.hwnd, wide("RustUsbBridge.CloseToDock").as_ptr());
            SetWindowLongPtrW(self.hwnd, GWLP_WNDPROC, self.previous);
            RemovePropW(self.hwnd, wide("RustUsbBridge.PreviousProc").as_ptr());
        }
    }
}
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if message == ACTIVATE {
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
        return 0;
    }
    if message == WM_CLOSE && !GetPropW(hwnd, wide("RustUsbBridge.CloseToDock").as_ptr()).is_null()
    {
        ShowWindow(
            hwnd,
            if IsIconic(hwnd) != 0 {
                SW_HIDE
            } else {
                SW_MINIMIZE
            },
        );
        return 0;
    }
    let previous = GetPropW(hwnd, wide("RustUsbBridge.PreviousProc").as_ptr());
    CallWindowProcW(
        Some(std::mem::transmute::<
            HANDLE,
            unsafe extern "system" fn(HWND, u32, usize, isize) -> isize,
        >(previous)),
        hwnd,
        message,
        wparam,
        lparam,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn close_moves_to_taskbar_then_tray_and_activation_restores() {
        unsafe {
            let hwnd = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("Lifecycle test").as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                200,
                100,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null());
            let hook = WindowHook::install(hwnd as isize, false, true).unwrap();
            for _ in 0..3 {
                SendMessageW(hwnd, ACTIVATE, 0, 0);
                assert_ne!(IsWindowVisible(hwnd), 0);
                assert_eq!(IsIconic(hwnd), 0);
                SendMessageW(hwnd, WM_CLOSE, 0, 0);
                assert_ne!(IsWindowVisible(hwnd), 0);
                assert_ne!(IsIconic(hwnd), 0);
                SendMessageW(hwnd, WM_CLOSE, 0, 0);
                assert_eq!(IsWindowVisible(hwnd), 0);
                assert_ne!(IsWindow(hwnd), 0);
            }
            hook.allow_exit();
            assert!(GetPropW(hwnd, wide("RustUsbBridge.CloseToDock").as_ptr()).is_null());
            drop(hook);
            DestroyWindow(hwnd);
        }
    }

    #[test]
    fn repeated_launch_signals_existing_window() {
        if std::env::var_os("USB_BRIDGE_INSTANCE_TEST_CHILD").is_some() {
            assert!(Instance::acquire(true).unwrap().is_none());
            return;
        }
        let instance = Instance::acquire(true)
            .unwrap()
            .expect("demo instance must be free for this test");
        unsafe {
            let hwnd = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("Instance test").as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                200,
                100,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null());
            let hook = WindowHook::install(hwnd as isize, true, true).unwrap();
            for _ in 0..3 {
                ShowWindow(hwnd, SW_HIDE);
                let status = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "window_lifecycle::tests::repeated_launch_signals_existing_window",
                    ])
                    .env("USB_BRIDGE_INSTANCE_TEST_CHILD", "1")
                    .status()
                    .unwrap();
                assert!(status.success());
                let mut message: MSG = std::mem::zeroed();
                assert_ne!(
                    PeekMessageW(&mut message, hwnd, ACTIVATE, ACTIVATE, PM_REMOVE),
                    0
                );
                DispatchMessageW(&message);
                assert_ne!(IsWindowVisible(hwnd), 0);
                assert_eq!(IsIconic(hwnd), 0);
            }
            drop(hook);
            DestroyWindow(hwnd);
        }
        drop(instance);
        assert!(
            Instance::acquire(true).unwrap().is_some(),
            "exit must release the single-instance guard"
        );
    }
}
