//! CRUSSTY module entry — the "not standalone" bootstrap.
//!
//! The distribution form factor is a single CRUSSTY module:
//!
//! ```text
//! modules/faster/
//! ├── module.json               # {"id":"faster","main":"libcrussty_chunkgen.so",...}
//! ├── libcrussty_chunkgen.so    # THIS library (cplugin_init + all JNI natives)
//! └── CrusstyGen.jar            # the Bukkit-side generator plugin (module resource)
//! ```
//!
//! The runtime dlopens the .so before the kernel boots and calls
//! `cplugin_init`. This module then:
//!   1. resolves its own .so path (dladdr on the exported symbol),
//!   2. locates the server root (cwd walk-up: server.properties / bukkit.yml /
//!      versions/ / modules/ / crus/),
//!   3. copies the bundled CrusstyGen.jar into <root>/plugins/ (byte-diff
//!      guarded — no pointless rewrites),
//!   4. writes <root>/plugins/CrusstyGen.crussty-native — the absolute .so
//!      path the plugin reads in its static init (System.load) so the JNI
//!      natives bind to the plugin's ClassLoader WITHOUT java.library.path.
//!
//! Standalone harness runs (probe/bench/CI) are untouched: there the jar is
//! deployed by harness scripts and ChunkGenNative falls back to
//! System.loadLibrary("crussty_chunkgen") via -Djava.library.path.
//!
//! Everything here is defensive (the release profile is panic=abort): no
//! unwrap on I/O paths, install failures are logged and skipped — the module
//! never aborts the kernel boot.

use cplug_abi::{CPluginApi, JavaVmPtr};
use std::ffi::{c_char, c_int, c_void, CStr};
use std::path::{Path, PathBuf};

pub const PLUGIN_JAR: &str = "CrusstyGen.jar";
pub const NATIVE_MARKER: &str = "CrusstyGen.crussty-native";

/// Source names searched next to the module lib. The SHIPPED name is
/// `CrusstyGen.jar.x`: the runtime's module scanner probes every *.jar/*.zip
/// under modules/ as an archive module (noise log + temp extraction per
/// boot), and `*.x` is the documented park-suffix it skips. The plain
/// `.jar` name is accepted for dev convenience.

/// dladdr handles — resolve the module's own absolute .so path.
#[repr(C)]
struct DlInfo {
    dli_fname: *const c_char,
    dli_fbase: *mut c_void,
    dli_sname: *const c_char,
    dli_saddr: *mut c_void,
}

extern "C" {
    fn dladdr(addr: *const c_void, info: *mut DlInfo) -> c_int;
}

/// Absolute path of the loaded module library, from the address of an
/// exported symbol of THIS cdylib.
fn own_so_path() -> Option<PathBuf> {
    unsafe {
        let mut info: DlInfo = std::mem::zeroed();
        if dladdr(cplugin_init as *const c_void, &mut info) == 0 || info.dli_fname.is_null() {
            return None;
        }
        let s = CStr::from_ptr(info.dli_fname).to_string_lossy().into_owned();
        if s.is_empty() {
            return None;
        }
        Some(PathBuf::from(s))
    }
}

/// Parse the runtime options string ("k=v;k2=v2") for the keys we own:
///   plugin=off|false|0   — skip the plugin install (natives-only mode)
///   plugin_dir=<path>    — override the target plugins/ directory
fn option<'a>(opts: &'a str, key: &str) -> Option<&'a str> {
    for part in opts.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == key {
                return Some(v.trim());
            }
        }
    }
    None
}

fn looks_like_server_root(p: &Path) -> bool {
    p.join("server.properties").is_file()
        || p.join("bukkit.yml").is_file()
        || p.join("versions").is_dir()
        || p.join("modules").is_dir()
        || p.join("crus").is_dir()
}

/// Find the server root: cwd, then up to two parents. Returns None outside
/// a server layout (probe/bench processes) — install is skipped then.
fn find_server_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut cand = cwd.clone();
    for _ in 0..3 {
        if looks_like_server_root(&cand) {
            return Some(cand);
        }
        cand = cand.parent()?.to_path_buf();
    }
    None
}

fn same_bytes(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Install the bundled plugin jar + the native-path marker into
/// <root>/plugins/. Pure filesystem work, idempotent.
fn install_plugin(jar_src: &Path, so_path: &Path, root: &Path) -> Result<(), String> {
    let plugins = root.join("plugins");
    std::fs::create_dir_all(&plugins).map_err(|e| format!("mkdir {}: {e}", plugins.display()))?;

    let jar_dst = plugins.join(PLUGIN_JAR);
    if !same_bytes(jar_src, &jar_dst) {
        std::fs::copy(jar_src, &jar_dst)
            .map_err(|e| format!("copy jar: {e}"))?;
    }

    let marker = plugins.join(NATIVE_MARKER);
    std::fs::write(&marker, format!("{}\n", so_path.display()))
        .map_err(|e| format!("write {}: {e}", marker.display()))?;
    Ok(())
}

/// The single required export (cplug-abi contract).
///
/// # Safety
/// `api` must be a valid CPluginApi owned by the runtime, `vm` the live
/// JavaVM pointer, `options` a NUL-terminated string for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cplugin_init(
    api: *const CPluginApi,
    vm: JavaVmPtr,
    options: *const c_char,
) -> i32 {
    cplug_sdk::init(api, vm);

    let opts = if options.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(options) }.to_string_lossy().into_owned()
    };

    let disabled = matches!(option(&opts, "plugin"), Some("off") | Some("false") | Some("0"));
    eprintln!(
        "[c-faster] cplugin_init: module loaded (plugin install {})",
        if disabled { "disabled by options" } else { "pending" }
    );
    if disabled {
        return 0;
    }

    let so = match own_so_path() {
        Some(p) => p,
        None => {
            eprintln!("[c-faster] dladdr failed — cannot resolve module path, install skipped");
            return 0;
        }
    };
    let so_dir = so.parent().map(|p| p.to_path_buf());
    let jar_src = ["CrusstyGen.jar.x", PLUGIN_JAR]
        .iter()
        .filter_map(|n| so_dir.as_ref().map(|d| d.join(n)))
        .find(|p| p.is_file());

    // Defer to a worker thread: cplugin_init runs on the runtime's OnLoad
    // thread before the VM is ready — keep it cheap (SDK house rule).
    std::thread::Builder::new()
        .name("cfaster-install".into())
        .spawn(move || {
            let root = match option(&opts, "plugin_dir") {
                Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
                _ => find_server_root(),
            };
            let Some(root) = root else {
                eprintln!(
                    "[c-faster] no server root detected (probe/bench mode?) — plugin install skipped"
                );
                return;
            };
            let Some(jar_src) = jar_src else {
                eprintln!(
                    "[c-faster] CrusstyGen.jar[.x] not found next to the module lib — install \
                     skipped (build.sh lays it out; standalone harness deploys its own jar)"
                );
                return;
            };
            match install_plugin(&jar_src, &so, &root) {
                Ok(()) => {
                    eprintln!(
                        "[c-faster] module install ok: {} + {} (natives: {})",
                        root.join("plugins").join(PLUGIN_JAR).display(),
                        NATIVE_MARKER,
                        so.display()
                    );
                    // Kernel-side confirmation once Bukkit exists.
                    cplug_sdk::on_kernel_ready("org.bukkit.Bukkit", || {
                        cplug_sdk::run_on_main_thread(|_env| {
                            cplug_sdk::log::info(
                                "[c-faster] module active: CrusstyGen plugin installed, natives bound",
                            );
                        });
                    });
                }
                Err(e) => eprintln!("[c-faster] module install FAILED: {e}"),
            }
        })
        .ok();

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_parse() {
        assert_eq!(option("a=1;plugin=off;b=2", "plugin"), Some("off"));
        assert_eq!(option("a=1", "plugin"), None);
        assert_eq!(option("plugin_dir=/srv/p;", "plugin_dir"), Some("/srv/p"));
        assert_eq!(option("plugin = 0", "plugin"), Some("0"));
        assert!(matches!(option("plugin=off", "plugin"), Some("off") | Some("false") | Some("0")));
    }

    #[test]
    fn root_detection_and_install() {
        let tmp = std::env::temp_dir().join(format!("cfaster-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let srv = tmp.join("srv");
        let modules = srv.join("modules");
        std::fs::create_dir_all(&modules).unwrap();
        let plugins = srv.join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();

        // cwd inside modules/ (the runtime's layout) — walk-up finds srv/.
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(&modules).unwrap();
        let root = find_server_root();
        std::env::set_current_dir(old).unwrap();
        assert_eq!(root.as_deref(), Some(srv.as_path()));

        // Install: jar + marker, idempotent.
        let jar = tmp.join("src.jar");
        std::fs::write(&jar, b"jarbytes").unwrap();
        let so = tmp.join("libcrussty_chunkgen.so");
        std::fs::write(&so, b"so-bytes").unwrap();
        install_plugin(&jar, &so, &srv).unwrap();
        let dst = plugins.join(PLUGIN_JAR);
        assert_eq!(std::fs::read(&dst).unwrap(), b"jarbytes");
        let marker = std::fs::read_to_string(plugins.join(NATIVE_MARKER)).unwrap();
        assert_eq!(marker.trim(), so.to_string_lossy());

        // Same bytes -> no rewrite (mtime untouched); different -> updated.
        let mtime0 = std::fs::metadata(&dst).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        install_plugin(&jar, &so, &srv).unwrap();
        assert_eq!(std::fs::metadata(&dst).unwrap().modified().unwrap(), mtime0);
        std::fs::write(&jar, b"NEW").unwrap();
        install_plugin(&jar, &so, &srv).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"NEW");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
