//! Android platform integration, in-memory HPK virtual filesystem, and input hooks.
#![cfg(target_os = "android")]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use winit::platform::android::activity::AndroidApp;

static APP: OnceLock<AndroidApp> = OnceLock::new();
static ARGS: OnceLock<Vec<String>> = OnceLock::new();

/// Store the AndroidApp handle for event loop creation.
pub fn set_android_app(app: AndroidApp) {
    let _ = APP.set(app);
}

/// Retrieve the AndroidApp handle.
pub fn android_app() -> AndroidApp {
    APP.get().expect("android_main sets the app first").clone()
}

/// The command line arguments configured for Android.
pub fn args() -> Vec<String> {
    ARGS.get().cloned().unwrap_or_default()
}

/// Parse command line arguments and environment variables from `horizon-args.txt`
/// or defaults (`--resources <home>/resources`).
fn command_line(home: &Path) -> (Vec<String>, Vec<(String, String)>) {
    let mut args = vec![
        "--resources".to_owned(),
        home.join("resources").display().to_string(),
    ];
    let mut env = Vec::new();
    let text = std::fs::read_to_string(home.join("horizon-args.txt")).unwrap_or_default();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_once('=') {
            Some((name, value))
                if !name.is_empty()
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') =>
            {
                env.push((name.to_owned(), value.to_owned()))
            }
            _ => args.push(line.to_owned()),
        }
    }
    (args, env)
}

/// Route log messages to Android logcat under tag "horizon".
pub fn logcat(level: log::Level, message: &str) {
    use std::ffi::CString;
    let priority = match level {
        log::Level::Error => 6,
        log::Level::Warn => 5,
        log::Level::Info => 4,
        log::Level::Debug => 3,
        log::Level::Trace => 2,
    };
    let tag = c"horizon";
    for line in message.lines() {
        let Ok(text) = CString::new(line.replace('\0', " ")) else {
            continue;
        };
        // SAFETY: both pointers are valid NUL-terminated strings.
        unsafe { __android_log_write(priority, tag.as_ptr(), text.as_ptr()) };
    }
}

fn log_line_hook(level: &str, _thread: &str, message: &str, _lines: &str) {
    let lvl = match level {
        "ERROR" => log::Level::Error,
        "WARN" => log::Level::Warn,
        "DEBUG" => log::Level::Debug,
        "TRACE" => log::Level::Trace,
        _ => log::Level::Info,
    };
    logcat(lvl, message);
}

/// Run `f` with the activity, on the calling thread (attached to the VM for the call).
fn with_activity<R>(
    f: impl FnOnce(&mut jni::JNIEnv, &jni::objects::JObject) -> jni::errors::Result<R>,
) -> Option<R> {
    let app = APP.get()?;
    // SAFETY: the VM and the activity are NativeActivity's, alive as long as the process; the
    // activity is a global reference, which this neither deletes nor keeps.
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };
    let result = f(&mut env, &activity);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    result.ok()
}

/// Capture the mouse for first-person camera look, or release it when in menus.
pub fn capture_pointer(capture: bool) {
    with_activity(|env, activity| {
        let window = env
            .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])?
            .l()?;
        let decor = env
            .call_method(window, "getDecorView", "()Landroid/view/View;", &[])?
            .l()?;
        let method = if capture {
            "requestPointerCapture"
        } else {
            "releasePointerCapture"
        };
        env.call_method(decor, method, "()V", &[])?;
        Ok(())
    });
}

/// The end of the game's run: finish the NativeActivity so Android doesn't leave
/// a black screen or zombie activity, then exit the process.
pub fn finish_and_exit() -> ! {
    with_activity(|env, activity| {
        let _ = env.call_method(activity, "finish", "()V", &[]);
        Ok(())
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    unsafe { libc::_exit(0) }
}

unsafe extern "C" {
    fn __android_log_write(
        priority: std::ffi::c_int,
        tag: *const std::ffi::c_char,
        text: *const std::ffi::c_char,
    ) -> std::ffi::c_int;
}

/// In-memory virtual filesystem backed by `resources.hpk` and a kernel `memfd` `horizon-data.bin`.
/// Intercepts libc file/directory calls (`--wrap`) for `<home>/resources` so zero upstream
/// resource loaders need Android-specific patches and zero bytes are unpacked to flash storage.
pub mod vfs {
    use super::{AndroidApp, logcat};
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::io::Read;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};

    const VDIR_FD_BASE: c_int = 0x4000_0000;
    const HZDATA_MAGIC: [u8; 8] = *b"HZDATA\0\0";
    const HZDATA_VERSION: u32 = 2;
    const HZDATA_HEADER: usize = 40;
    const HZDATA_ENTRY: usize = 24;
    const HZDATA_FOLDERS: [&str; 3] = ["data", "assets", "worldgen/structure"];

    enum FileData {
        /// Stored inside `hzdata_fd` at `(offset, len)`.
        HzData { offset: u64, len: usize },
        /// Stored directly in memory (top-level `.json`/`.nbt` tables and `.ogg` sounds).
        Bytes(Box<[u8]>),
    }

    impl FileData {
        fn len(&self) -> usize {
            match self {
                Self::HzData { len, .. } => *len,
                Self::Bytes(b) => b.len(),
            }
        }
    }

    struct VirtualDir {
        vfd: c_int,
        entries: Vec<(String, u8)>,
        index: usize,
        dirent: libc::dirent,
    }

    struct Vfs {
        roots: Vec<String>,
        hzdata_fd: c_int,
        hzdata_len: u64,
        files: HashMap<String, FileData>,
        dirs: HashMap<String, Vec<(String, u8)>>,
        next_vfd: Mutex<c_int>,
        open_vdirs: Mutex<HashMap<c_int, String>>,
        open_dirps: Mutex<HashMap<usize, Box<VirtualDir>>>,
    }

    static VFS: OnceLock<Vfs> = OnceLock::new();

    fn holds_folder(dir: &str) -> bool {
        HZDATA_FOLDERS
            .iter()
            .any(|f| dir == *f || dir.strip_prefix(f).is_some_and(|r| r.starts_with('/')))
    }

    fn holds_file(name: &str) -> bool {
        holds_folder(name) && !name.ends_with(".ogg")
    }

    fn pad8(n: u64) -> u64 {
        (n + 7) & !7
    }

    fn create_memfd(name: &CStr) -> c_int {
        unsafe { libc::syscall(libc::SYS_memfd_create, name.as_ptr(), libc::MFD_CLOEXEC) as c_int }
    }

    fn write_all_fd(fd: c_int, mut buf: &[u8]) -> bool {
        while !buf.is_empty() {
            let n = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
            if n <= 0 {
                return false;
            }
            buf = &buf[n as usize..];
        }
        true
    }

    fn write_padded_fd(fd: c_int, bytes: &[u8]) -> bool {
        let zeros = [0u8; 8];
        let pad = (pad8(bytes.len() as u64) as usize) - bytes.len();
        write_all_fd(fd, bytes) && (pad == 0 || write_all_fd(fd, &zeros[..pad]))
    }

    fn decompress_hpk_stream(
        compressed: &[u8],
        raw_files: &mut BTreeMap<String, Vec<u8>>,
    ) -> Result<usize, String> {
        let mut decompressed = Vec::new();
        let mut cursor = std::io::Cursor::new(compressed);
        lzma_rs::xz_decompress(&mut cursor, &mut decompressed)
            .map_err(|e| format!("XZ decompression error: {e}"))?;

        let mut cur = std::io::Cursor::new(decompressed);
        let mut count_buf = [0u8; 4];
        cur.read_exact(&mut count_buf).map_err(|e| e.to_string())?;
        let count = u32::from_le_bytes(count_buf) as usize;

        let decomp_bytes = cur.into_inner();
        let total_len = decomp_bytes.len();
        let mut pos = 4;

        for _ in 0..count {
            if pos + 2 > total_len {
                break;
            }
            let name_len = u16::from_le_bytes([decomp_bytes[pos], decomp_bytes[pos + 1]]) as usize;
            pos += 2;
            if pos + name_len + 4 > total_len {
                break;
            }
            let name = std::str::from_utf8(&decomp_bytes[pos..pos + name_len])
                .map_err(|e| e.to_string())?
                .to_owned();
            pos += name_len;
            let data_len =
                u32::from_le_bytes(decomp_bytes[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            if pos + data_len > total_len {
                break;
            }
            raw_files.insert(name, decomp_bytes[pos..pos + data_len].to_vec());
            pos += data_len;
        }
        Ok(count)
    }

    pub fn load_hpk_and_mount(app: &AndroidApp, target: &Path) {
        let _ = std::fs::remove_dir_all(target);
        let _ = std::fs::create_dir_all(target);

        let start = std::time::Instant::now();
        let mut hpk_data = None;
        for name in [c"resources.hpk", c"assets/resources.hpk"] {
            if let Some(mut asset) = app.asset_manager().open(name) {
                let buf = match asset.buffer() {
                    Ok(b) => b.to_vec(),
                    Err(_) => {
                        let mut b = Vec::new();
                        let _ = std::io::copy(&mut asset, &mut b);
                        b
                    }
                };
                if !buf.is_empty() {
                    hpk_data = Some(buf);
                    break;
                }
            }
        }

        let Some(hpk_bytes) = hpk_data else {
            logcat(log::Level::Error, "resources.hpk not found in APK assets!");
            return;
        };
        if hpk_bytes.len() < 8 || &hpk_bytes[..4] != b"HPK1" {
            logcat(log::Level::Error, "Invalid HPK1 header in resources.hpk");
            return;
        }

        let num_streams = u32::from_le_bytes(hpk_bytes[4..8].try_into().unwrap()) as usize;
        let mut pos = 8;
        let mut raw_files = BTreeMap::new();

        for i in 0..num_streams {
            if pos + 8 > hpk_bytes.len() {
                break;
            }
            let comp_len = u32::from_le_bytes(hpk_bytes[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 8;
            if pos + comp_len > hpk_bytes.len() {
                break;
            }
            if let Err(e) = decompress_hpk_stream(&hpk_bytes[pos..pos + comp_len], &mut raw_files) {
                logcat(log::Level::Error, &format!("HPK stream {i} failed: {e}"));
                return;
            }
            pos += comp_len;
        }
        drop(hpk_bytes);

        // Build directory index for every directory from "" (root) down.
        let mut dir_sets: HashMap<String, BTreeSet<(String, u8)>> = HashMap::new();
        dir_sets.entry(String::new()).or_default();

        for rel in raw_files.keys().cloned().collect::<Vec<_>>() {
            let mut cur = rel.as_str();
            let mut is_child_dir = false;
            while let Some((parent, child)) = cur.rsplit_once('/') {
                let dtype = if is_child_dir {
                    libc::DT_DIR
                } else {
                    libc::DT_REG
                };
                dir_sets
                    .entry(parent.to_owned())
                    .or_default()
                    .insert((child.to_owned(), dtype));
                cur = parent;
                is_child_dir = true;
            }
            let top_dtype = if is_child_dir {
                libc::DT_DIR
            } else {
                libc::DT_REG
            };
            dir_sets
                .entry(String::new())
                .or_default()
                .insert((cur.to_owned(), top_dtype));
        }
        dir_sets
            .entry(String::new())
            .or_default()
            .insert(("horizon-data.bin".to_owned(), libc::DT_REG));

        let dirs: HashMap<String, Vec<(String, u8)>> = dir_sets
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().collect()))
            .collect();

        // Build `horizon-data.bin` in a kernel memfd so upstream `horizon_core::prepared_files`
        // works out-of-the-box without modifying `prepared_files.rs` and without duplicating
        // the 22,000 small files in heap memory.
        let canonical = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
        let root_bytes = canonical.as_os_str().as_encoded_bytes();
        let source_bytes = raw_files
            .get("horizon-resources.json")
            .cloned()
            .unwrap_or_default();

        let hz_count = raw_files.keys().filter(|k| holds_file(k)).count();
        let pool_len: usize = raw_files
            .keys()
            .filter(|k| holds_file(k))
            .map(String::len)
            .sum();
        let data_start = HZDATA_HEADER as u64
            + pad8(root_bytes.len() as u64)
            + pad8(source_bytes.len() as u64)
            + (hz_count * HZDATA_ENTRY) as u64
            + pad8(pool_len as u64);

        let hzdata_fd = create_memfd(c"horizon-data.bin");
        let mut header = Vec::with_capacity(HZDATA_HEADER);
        header.extend_from_slice(&HZDATA_MAGIC);
        header.extend_from_slice(&HZDATA_VERSION.to_le_bytes());
        header.extend_from_slice(&(hz_count as u32).to_le_bytes());
        for len in [root_bytes.len(), source_bytes.len(), pool_len] {
            header.extend_from_slice(&(len as u64).to_le_bytes());
        }
        write_all_fd(hzdata_fd, &header);
        write_padded_fd(hzdata_fd, root_bytes);
        write_padded_fd(hzdata_fd, &source_bytes);

        let mut entry_table = Vec::with_capacity(hz_count * HZDATA_ENTRY);
        let mut name_pool = Vec::with_capacity(pool_len);
        let (mut name_at, mut data_at) = (0u32, data_start);
        for (name, bytes) in &raw_files {
            if !holds_file(name) {
                continue;
            }
            entry_table.extend_from_slice(&name_at.to_le_bytes());
            entry_table.extend_from_slice(&(name.len() as u32).to_le_bytes());
            entry_table.extend_from_slice(&data_at.to_le_bytes());
            entry_table.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            name_pool.extend_from_slice(name.as_bytes());
            name_at += name.len() as u32;
            data_at += bytes.len() as u64;
        }
        write_all_fd(hzdata_fd, &entry_table);
        drop(entry_table);
        write_padded_fd(hzdata_fd, &name_pool);
        drop(name_pool);

        let mut files = HashMap::with_capacity(raw_files.len());
        let mut cur_data_at = data_start;
        for (name, bytes) in raw_files {
            if holds_file(&name) {
                let len = bytes.len();
                write_all_fd(hzdata_fd, &bytes);
                files.insert(
                    name,
                    FileData::HzData {
                        offset: cur_data_at,
                        len,
                    },
                );
                cur_data_at += len as u64;
            } else {
                files.insert(name, FileData::Bytes(bytes.into_boxed_slice()));
            }
        }
        unsafe { libc::lseek(hzdata_fd, 0, libc::SEEK_SET) };

        let mut roots = vec![target.to_string_lossy().trim_end_matches('/').to_owned()];
        let canon_str = canonical.to_string_lossy().trim_end_matches('/').to_owned();
        if !roots.contains(&canon_str) {
            roots.push(canon_str);
        }

        let total_files = files.len();
        let _ = VFS.set(Vfs {
            roots,
            hzdata_fd,
            hzdata_len: cur_data_at,
            files,
            dirs,
            next_vfd: Mutex::new(VDIR_FD_BASE),
            open_vdirs: Mutex::new(HashMap::new()),
            open_dirps: Mutex::new(HashMap::new()),
        });

        logcat(
            log::Level::Info,
            &format!(
                "Mounted {total_files} HPK assets in memory ({:.2} MiB horizon-data.bin memfd) in {:.2}s",
                cur_data_at as f64 / (1024.0 * 1024.0),
                start.elapsed().as_secs_f32()
            ),
        );
    }

    fn normalize_rel(s: &str) -> String {
        let mut parts = Vec::new();
        for part in s.split('/') {
            match part {
                "" | "." => continue,
                ".." => {
                    parts.pop();
                }
                p => parts.push(p),
            }
        }
        parts.join("/")
    }

    fn resolve_path(vfs: &Vfs, dirfd: c_int, c_path: *const c_char) -> Option<String> {
        if c_path.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(c_path) }.to_str().ok()?;
        if s.starts_with('/') {
            for root in &vfs.roots {
                if s == root {
                    return Some(String::new());
                }
                if let Some(rest) = s.strip_prefix(root)
                    && let Some(rel) = rest.strip_prefix('/')
                {
                    return Some(normalize_rel(rel));
                }
            }
            return None;
        }
        if dirfd >= VDIR_FD_BASE {
            let vdirs = vfs.open_vdirs.lock().unwrap_or_else(|e| e.into_inner());
            let base = vdirs.get(&dirfd)?;
            let joined = if base.is_empty() {
                s.to_owned()
            } else {
                format!("{base}/{s}")
            };
            return Some(normalize_rel(&joined));
        }
        if dirfd == libc::AT_FDCWD {
            let trimmed = s.strip_prefix("./").unwrap_or(s);
            if trimmed == "resources" {
                return Some(String::new());
            }
            if let Some(rel) = trimmed.strip_prefix("resources/") {
                return Some(normalize_rel(rel));
            }
        }
        None
    }

    fn alloc_vfd(vfs: &Vfs, rel: String) -> c_int {
        let mut next = vfs.next_vfd.lock().unwrap_or_else(|e| e.into_inner());
        let vfd = *next;
        *next += 1;
        vfs.open_vdirs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(vfd, rel);
        vfd
    }

    pub unsafe fn open(dirfd: c_int, path: *const c_char, flags: c_int) -> Option<c_int> {
        let vfs = VFS.get()?;
        if flags & (libc::O_WRONLY | libc::O_RDWR | libc::O_CREAT | libc::O_TRUNC | libc::O_APPEND)
            != 0
        {
            return None;
        }
        let rel = resolve_path(vfs, dirfd, path)?;
        if rel == "horizon-data.bin" {
            let proc_path = CString::new(format!("/proc/self/fd/{}", vfs.hzdata_fd)).ok()?;
            let fd = unsafe {
                libc::syscall(
                    libc::SYS_openat,
                    libc::AT_FDCWD,
                    proc_path.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC,
                    0,
                ) as c_int
            };
            if fd >= 0 {
                return Some(fd);
            }
            unsafe { libc::lseek(vfs.hzdata_fd, 0, libc::SEEK_SET) };
            return Some(unsafe { libc::dup(vfs.hzdata_fd) });
        }
        if let Some(file) = vfs.files.get(&rel) {
            let fd = create_memfd(c"hpk-file");
            if fd < 0 {
                return Some(-1);
            }
            match file {
                FileData::Bytes(bytes) => {
                    write_all_fd(fd, bytes);
                }
                FileData::HzData { offset, len } => {
                    let mut buf = vec![0u8; *len];
                    let mut read_total = 0usize;
                    while read_total < *len {
                        let n = unsafe {
                            libc::pread64(
                                vfs.hzdata_fd,
                                buf[read_total..].as_mut_ptr().cast(),
                                *len - read_total,
                                (*offset + read_total as u64) as libc::off64_t,
                            )
                        };
                        if n <= 0 {
                            break;
                        }
                        read_total += n as usize;
                    }
                    write_all_fd(fd, &buf[..read_total]);
                }
            }
            unsafe { libc::lseek(fd, 0, libc::SEEK_SET) };
            return Some(fd);
        }
        if vfs.dirs.contains_key(&rel) {
            return Some(alloc_vfd(vfs, rel));
        }
        unsafe { *libc::__errno() = libc::ENOENT };
        Some(-1)
    }

    fn fill_stat(buf: *mut libc::stat, mode: libc::mode_t, size: i64) {
        if buf.is_null() {
            return;
        }
        unsafe {
            std::ptr::write_bytes(buf, 0, 1);
            (*buf).st_mode = mode;
            (*buf).st_size = size;
            (*buf).st_nlink = 1;
            (*buf).st_blksize = 4096;
        }
    }

    pub unsafe fn stat(dirfd: c_int, path: *const c_char, buf: *mut libc::stat) -> Option<c_int> {
        let vfs = VFS.get()?;
        let rel = resolve_path(vfs, dirfd, path)?;
        if rel.is_empty() {
            // Let the real filesystem stat the `<home>/resources` mount directory itself.
            return None;
        }
        if rel == "horizon-data.bin" {
            fill_stat(buf, libc::S_IFREG | 0o644, vfs.hzdata_len as i64);
            return Some(0);
        }
        if let Some(file) = vfs.files.get(&rel) {
            fill_stat(buf, libc::S_IFREG | 0o644, file.len() as i64);
            return Some(0);
        }
        if vfs.dirs.contains_key(&rel) {
            fill_stat(buf, libc::S_IFDIR | 0o755, 4096);
            return Some(0);
        }
        unsafe { *libc::__errno() = libc::ENOENT };
        Some(-1)
    }

    pub unsafe fn fstat(fd: c_int, buf: *mut libc::stat) -> Option<c_int> {
        let vfs = VFS.get()?;
        if fd >= VDIR_FD_BASE {
            let vdirs = vfs.open_vdirs.lock().unwrap_or_else(|e| e.into_inner());
            if vdirs.contains_key(&fd) {
                fill_stat(buf, libc::S_IFDIR | 0o755, 4096);
                return Some(0);
            }
        }
        None
    }

    pub unsafe fn access(path: *const c_char) -> Option<c_int> {
        let vfs = VFS.get()?;
        let rel = resolve_path(vfs, libc::AT_FDCWD, path)?;
        if rel.is_empty()
            || rel == "horizon-data.bin"
            || vfs.files.contains_key(&rel)
            || vfs.dirs.contains_key(&rel)
        {
            return Some(0);
        }
        unsafe { *libc::__errno() = libc::ENOENT };
        Some(-1)
    }

    fn make_dirp(vfs: &Vfs, vfd: c_int, entries: Vec<(String, u8)>) -> *mut c_void {
        let mut vdir = Box::new(VirtualDir {
            vfd,
            entries,
            index: 0,
            dirent: unsafe { std::mem::zeroed() },
        });
        let ptr = (&mut *vdir as *mut VirtualDir).cast::<c_void>();
        vfs.open_dirps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(ptr as usize, vdir);
        ptr
    }

    pub unsafe fn opendir(path: *const c_char) -> Option<*mut c_void> {
        let vfs = VFS.get()?;
        let rel = resolve_path(vfs, libc::AT_FDCWD, path)?;
        if let Some(entries) = vfs.dirs.get(&rel) {
            let vfd = alloc_vfd(vfs, rel);
            return Some(make_dirp(vfs, vfd, entries.clone()));
        }
        unsafe { *libc::__errno() = libc::ENOENT };
        Some(std::ptr::null_mut())
    }

    pub fn fdopendir(fd: c_int) -> Option<*mut c_void> {
        let vfs = VFS.get()?;
        if fd < VDIR_FD_BASE {
            return None;
        }
        let rel = vfs
            .open_vdirs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&fd)
            .cloned()?;
        let entries = vfs.dirs.get(&rel)?.clone();
        Some(make_dirp(vfs, fd, entries))
    }

    pub unsafe fn readdir(dirp: *mut c_void) -> Option<*mut libc::dirent> {
        let vfs = VFS.get()?;
        let mut dirps = vfs.open_dirps.lock().unwrap_or_else(|e| e.into_inner());
        let vdir = dirps.get_mut(&(dirp as usize))?;
        if vdir.index >= vdir.entries.len() {
            return Some(std::ptr::null_mut());
        }
        let (name, dtype) = &vdir.entries[vdir.index];
        vdir.index += 1;
        vdir.dirent = unsafe { std::mem::zeroed() };
        vdir.dirent.d_ino = vdir.index as u64;
        vdir.dirent.d_off = vdir.index as i64;
        vdir.dirent.d_reclen = std::mem::size_of::<libc::dirent>() as u16;
        vdir.dirent.d_type = *dtype;
        let bytes = name.as_bytes();
        let copy_len = bytes.len().min(vdir.dirent.d_name.len() - 1);
        for (i, &b) in bytes[..copy_len].iter().enumerate() {
            vdir.dirent.d_name[i] = b as c_char;
        }
        vdir.dirent.d_name[copy_len] = 0;
        Some(&mut vdir.dirent as *mut libc::dirent)
    }

    pub unsafe fn closedir(dirp: *mut c_void) -> Option<c_int> {
        let vfs = VFS.get()?;
        let vdir = vfs
            .open_dirps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(dirp as usize))?;
        vfs.open_vdirs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&vdir.vfd);
        Some(0)
    }

    pub unsafe fn dirfd(dirp: *mut c_void) -> Option<c_int> {
        let vfs = VFS.get()?;
        let dirps = vfs.open_dirps.lock().unwrap_or_else(|e| e.into_inner());
        let vdir = dirps.get(&(dirp as usize))?;
        Some(vdir.vfd)
    }

    pub fn close(fd: c_int) -> Option<c_int> {
        let vfs = VFS.get()?;
        if fd >= VDIR_FD_BASE {
            vfs.open_vdirs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&fd);
            return Some(0);
        }
        None
    }
}

fn log_storage_census(home: &Path) {
    let mut entries = Vec::new();
    let mut total_bytes = 0u64;
    fn scan(dir: &Path, rel: &str, total: &mut u64, entries: &mut Vec<String>) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                let name = format!("{rel}/{}", e.file_name().to_string_lossy());
                if let Ok(m) = e.metadata() {
                    if m.is_dir() {
                        scan(&p, &name, total, entries);
                    } else {
                        *total += m.len();
                        entries.push(format!("{name}: {} bytes ({:.2} KB)", m.len(), m.len() as f64 / 1024.0));
                    }
                }
            }
        }
    }
    let app_root = home.parent().unwrap_or(home);
    scan(app_root, "", &mut total_bytes, &mut entries);
    let summary = format!(
        "Storage census: total {total_bytes} bytes ({:.2} MB):\n{}",
        total_bytes as f64 / (1024.0 * 1024.0),
        entries.join("\n")
    );
    logcat(log::Level::Info, &summary);
    let _ = std::fs::write("/sdcard/Download/horizon_storage_census.txt", summary);
}

/// Entry point called by `android_main` in horizon-android.
pub fn main(app: AndroidApp) {
    unsafe {
        libc::pthread_setname_np(libc::pthread_self(), c"main".as_ptr());
    }
    set_android_app(app.clone());
    horizon_client_render::logging::set_line_hook(log_line_hook);

    let home: PathBuf = app
        .internal_data_path()
        .unwrap_or_else(|| PathBuf::from("/data/local/tmp/horizon"));
    let _ = std::fs::create_dir_all(&home);

    winit::platform::android::set_input_hook(input::hook);
    vfs::load_hpk_and_mount(&app, &home.join("resources"));

    let (cli_args, env) = command_line(&home);
    unsafe {
        std::env::set_var("HORIZON_HOME", &home);
        std::env::set_var("HOME", &home);
        // On mobile, default HDR to none and GPU cull to 0 unless overridden in horizon-args.txt.
        // This avoids compiling unneeded HDR tonemapping pipelines and GPU compute cull shaders,
        // reducing Qualcomm Vulkan driver pipeline cache from ~3MB down to ~800KB.
        if std::env::var_os("HORIZON_HDR_OUTPUT").is_none() {
            std::env::set_var("HORIZON_HDR_OUTPUT", "none");
        }
        if std::env::var_os("HORIZON_GPU_CULL").is_none() {
            std::env::set_var("HORIZON_GPU_CULL", "0");
        }
        for (name, value) in &env {
            std::env::set_var(name, value);
        }
    }
    let _ = std::env::set_current_dir(&home);
    let _ = ARGS.set(cli_args);

    let census_home = home.clone();
    let _ = std::thread::Builder::new()
        .name("storage-census".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            log_storage_census(&census_home);
        });

    crate::entry::main();
}

pub mod input {
    use std::sync::Mutex;
    use winit::event::MouseButton;
    use winit::platform::android::activity::{
        InputStatus,
        input::{Axis, InputEvent, MotionAction, MotionEvent},
    };

    const SOURCE_MOUSE: u32 = 0x2000;
    const SOURCE_MOUSE_RELATIVE: u32 = 0x0002_0000;
    const BUTTONS: [(u32, MouseButton); 3] = [
        (1, MouseButton::Left),
        (2, MouseButton::Right),
        (4, MouseButton::Middle),
    ];
    const MOUSE_QUEUE: usize = 512;

    #[derive(Debug)]
    pub enum MouseEvent {
        Moved(f64, f64),
        Button(MouseButton, bool),
        Wheel(f32, f32),
        Motion(f64, f64),
    }

    struct Mouse {
        events: Vec<MouseEvent>,
        buttons: u32,
        at: Option<(f32, f32)>,
    }

    static MOUSE: Mutex<Mouse> = Mutex::new(Mouse {
        events: Vec::new(),
        buttons: 0,
        at: None,
    });

    pub fn hook(event: &InputEvent<'_>) -> Option<InputStatus> {
        match event {
            InputEvent::MotionEvent(motion) => motion_input(motion),
            _ => None,
        }
    }

    fn motion_input(motion: &MotionEvent<'_>) -> Option<InputStatus> {
        let source: u32 = motion.source().into();
        let relative = source & SOURCE_MOUSE_RELATIVE != 0;
        let is_mouse = (source & SOURCE_MOUSE != 0) || relative;
        let is_touch = (source & 0x1002) != 0;

        if !is_mouse && !is_touch {
            return None;
        }

        if motion.pointer_count() == 0 {
            return Some(InputStatus::Handled);
        }

        let mut mouse = MOUSE.lock().unwrap_or_else(|e| e.into_inner());
        let mut events = Vec::new();
        let pointer = motion.pointer_at_index(0);

        if is_touch {
            let at = (pointer.x(), pointer.y());
            match motion.action() {
                MotionAction::Down => {
                    events.push(MouseEvent::Moved(f64::from(at.0), f64::from(at.1)));
                    events.push(MouseEvent::Button(MouseButton::Left, true));
                }
                MotionAction::Move => {
                    events.push(MouseEvent::Moved(f64::from(at.0), f64::from(at.1)));
                }
                MotionAction::Up | MotionAction::Cancel => {
                    events.push(MouseEvent::Button(MouseButton::Left, false));
                }
                _ => {}
            }
        } else {
            let prev_at = mouse.at;
            let at = (pointer.x(), pointer.y());
            if !relative && motion.action() != MotionAction::HoverExit {
                if mouse.at != Some(at) {
                    mouse.at = Some(at);
                    events.push(MouseEvent::Moved(f64::from(at.0), f64::from(at.1)));
                }
            }

            let (x, y) = if relative {
                (Axis::X, Axis::Y)
            } else {
                (Axis::RelativeX, Axis::RelativeY)
            };
            let (mut dx, mut dy) = (pointer.axis_value(x), pointer.axis_value(y));
            for earlier in pointer.history() {
                dx += earlier.axis_value(x);
                dy += earlier.axis_value(y);
            }
            if dx == 0.0 && dy == 0.0 && !relative {
                if let Some(prev) = prev_at {
                    let fdx = at.0 - prev.0;
                    let fdy = at.1 - prev.1;
                    if fdx != 0.0 || fdy != 0.0 {
                        dx = fdx;
                        dy = fdy;
                    }
                }
            }
            if dx != 0.0 || dy != 0.0 {
                events.push(MouseEvent::Motion(f64::from(dx), f64::from(dy)));
            }

            if motion.action() == MotionAction::Scroll {
                let right = pointer.axis_value(Axis::Hscroll);
                let up = pointer.axis_value(Axis::Vscroll);
                if right != 0.0 || up != 0.0 {
                    events.push(MouseEvent::Wheel(-right, up));
                }
            }

            let buttons = motion.button_state().0;
            for (bit, button) in BUTTONS {
                if (buttons ^ mouse.buttons) & bit != 0 {
                    events.push(MouseEvent::Button(button, buttons & bit != 0));
                }
            }
            mouse.buttons = buttons;
        }

        if mouse.events.len() + events.len() <= MOUSE_QUEUE {
            mouse.events.append(&mut events);
        }
        Some(InputStatus::Handled)
    }

    pub fn take_mouse_events() -> Vec<MouseEvent> {
        let mut mouse = MOUSE.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut mouse.events)
    }
}
pub use input::{MouseEvent, take_mouse_events};
