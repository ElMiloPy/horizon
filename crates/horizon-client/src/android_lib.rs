//! Android cdylib entry point and libc VFS symbol wrappers.
#![cfg(target_os = "android")]

use std::ffi::{c_char, c_int, c_uint, c_void};
use winit::platform::android::activity::AndroidApp;

unsafe extern "C" {
    fn setenv(name: *const c_char, value: *const c_char, overwrite: c_int) -> c_int;

    pub fn __real_open(path: *const c_char, flags: c_int, mode: c_uint) -> c_int;
    pub fn __real_openat(dirfd: c_int, path: *const c_char, flags: c_int, mode: c_uint) -> c_int;
    pub fn __real_stat(path: *const c_char, buf: *mut libc::stat) -> c_int;
    pub fn __real_lstat(path: *const c_char, buf: *mut libc::stat) -> c_int;
    pub fn __real_fstat(fd: c_int, buf: *mut libc::stat) -> c_int;
    pub fn __real_fstatat(
        dirfd: c_int,
        path: *const c_char,
        buf: *mut libc::stat,
        flag: c_int,
    ) -> c_int;
    pub fn __real_access(path: *const c_char, mode: c_int) -> c_int;
    pub fn __real_opendir(path: *const c_char) -> *mut libc::DIR;
    pub fn __real_fdopendir(fd: c_int) -> *mut libc::DIR;
    pub fn __real_readdir(dirp: *mut libc::DIR) -> *mut libc::dirent;
    pub fn __real_closedir(dirp: *mut libc::DIR) -> c_int;
    pub fn __real_dirfd(dirp: *mut libc::DIR) -> c_int;
    pub fn __real_close(fd: c_int) -> c_int;
}

extern "C" fn init_android_stack_size() {
    // Android's default pthread stack is 1 MiB, whereas `ANativeActivity_onCreate`
    // spawns `android_main` via `std::thread::spawn` and `Game` startup frames
    // use ~1.4 MiB. Setting `RUST_MIN_STACK` before `ANativeActivity_onCreate`
    // gives `android_main` and all worker/server threads a 16 MiB stack.
    unsafe {
        setenv(c"RUST_MIN_STACK".as_ptr(), c"16777216".as_ptr(), 1);
    }
}

#[used]
#[unsafe(link_section = ".init_array")]
static INIT_ANDROID_STACK_SIZE: extern "C" fn() = init_android_stack_size;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_open(path: *const c_char, flags: c_int, mode: c_uint) -> c_int {
    if let Some(res) = unsafe { horizon_client::android::vfs::open(libc::AT_FDCWD, path, flags) } {
        return res;
    }
    unsafe { __real_open(path, flags, mode) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_openat(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mode: c_uint,
) -> c_int {
    if let Some(res) = unsafe { horizon_client::android::vfs::open(dirfd, path, flags) } {
        return res;
    }
    unsafe { __real_openat(dirfd, path, flags, mode) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_stat(path: *const c_char, buf: *mut libc::stat) -> c_int {
    if let Some(res) = unsafe { horizon_client::android::vfs::stat(libc::AT_FDCWD, path, buf) } {
        return res;
    }
    unsafe { __real_stat(path, buf) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_lstat(path: *const c_char, buf: *mut libc::stat) -> c_int {
    if let Some(res) = unsafe { horizon_client::android::vfs::stat(libc::AT_FDCWD, path, buf) } {
        return res;
    }
    unsafe { __real_lstat(path, buf) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_fstat(fd: c_int, buf: *mut libc::stat) -> c_int {
    if let Some(res) = unsafe { horizon_client::android::vfs::fstat(fd, buf) } {
        return res;
    }
    unsafe { __real_fstat(fd, buf) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_fstatat(
    dirfd: c_int,
    path: *const c_char,
    buf: *mut libc::stat,
    flag: c_int,
) -> c_int {
    if let Some(res) = unsafe { horizon_client::android::vfs::stat(dirfd, path, buf) } {
        return res;
    }
    unsafe { __real_fstatat(dirfd, path, buf, flag) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_access(path: *const c_char, mode: c_int) -> c_int {
    if let Some(res) = unsafe { horizon_client::android::vfs::access(path) } {
        return res;
    }
    unsafe { __real_access(path, mode) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_opendir(path: *const c_char) -> *mut libc::DIR {
    if let Some(res) = unsafe { horizon_client::android::vfs::opendir(path) } {
        return res.cast();
    }
    unsafe { __real_opendir(path) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_fdopendir(fd: c_int) -> *mut libc::DIR {
    if let Some(res) = horizon_client::android::vfs::fdopendir(fd) {
        return res.cast();
    }
    unsafe { __real_fdopendir(fd) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_readdir(dirp: *mut libc::DIR) -> *mut libc::dirent {
    if let Some(res) = unsafe { horizon_client::android::vfs::readdir(dirp.cast::<c_void>()) } {
        return res;
    }
    unsafe { __real_readdir(dirp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_closedir(dirp: *mut libc::DIR) -> c_int {
    if let Some(res) = unsafe { horizon_client::android::vfs::closedir(dirp.cast::<c_void>()) } {
        return res;
    }
    unsafe { __real_closedir(dirp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_dirfd(dirp: *mut libc::DIR) -> c_int {
    if let Some(res) = unsafe { horizon_client::android::vfs::dirfd(dirp.cast::<c_void>()) } {
        return res;
    }
    unsafe { __real_dirfd(dirp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_close(fd: c_int) -> c_int {
    if let Some(res) = horizon_client::android::vfs::close(fd) {
        return res;
    }
    unsafe { __real_close(fd) }
}

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    horizon_client::android::main(app);
}
