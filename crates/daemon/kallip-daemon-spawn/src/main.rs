//! Detach-and-exec helper for the kallip local daemon.
//!
//! Fixed argv, zero protocol, no serde, libc only:
//!
//! ```text
//! kallip-daemon-spawn <instance-dir> <exec-path> [KEY=VALUE ...]
//! ```
//!
//! The daemon runs this helper; the helper double-forks (fork → setsid →
//! fork) so the exec'd instance survives the daemon and holds no controlling
//! terminal, applies the allowlisted KEY=VALUE tail args as the child env,
//! then execs the target. The instance writes its own `runtime.json` into
//! `<instance-dir>` (its `KALLIP_DATA_DIR`) for the daemon to adopt.
//!
//! Dependency isolation is the point of a separate crate: this binary is the
//! setuid-root candidate of the packaged install, so its audit surface
//! stays std + libc, nothing else. In the unpackaged same-uid profile
//! the helper runs unprivileged; `--uid`/`--gid` exist so the packaged
//! setuid install reuses the same code path.
//!
//! `KEY=VALUE` args pass through UNFILTERED here; the frozen allowlist
//! check moves into this helper at packaging time, where the setuid
//! binary cannot trust its caller.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

const USAGE: &str = "usage: kallip-daemon-spawn <instance-dir> <exec-path> [KEY=VALUE ...]";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(&args);
    std::process::exit(code);
}

fn run(args: &[String]) -> i32 {
    // Fixed argv grammar: exactly one positional pair, then KEY=VALUE pairs.
    let [dir, exec, envs @ ..] = args else {
        eprintln!("{USAGE}");
        return 64; // EX_USAGE
    };
    for pair in envs {
        if !pair.contains('=') || pair.is_empty() {
            eprintln!("env arg {pair:?} is not KEY=VALUE");
            return 64;
        }
    }
    let instance_dir = Path::new(dir);
    if !instance_dir.is_dir() {
        eprintln!("instance dir {dir:?} is not a directory");
        return 66; // EX_NOINPUT
    }
    let exec_path = Path::new(exec);
    if !exec_path.is_file() {
        eprintln!("exec target {exec:?} is not a file");
        return 66;
    }
    // Resolve the exec target against the CURRENT directory before the
    // chdir into the instance dir: a relative caller path must keep
    // meaning after the grandchild changes directories.
    let exec_abs = std::fs::canonicalize(exec_path).unwrap_or_else(|_| exec_path.to_path_buf());
    match detach_and_exec(instance_dir, &exec_abs, envs) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("kallip-daemon-spawn: {message}");
            71 // EX_OSERR
        }
    }
}

/// Double-fork + exec. Returns Ok on successful detach (the grandchild's
/// exec failures are invisible to us by design — the daemon's 30s pidfile
/// wait is what turns a failed exec into a rollback).
fn detach_and_exec(instance_dir: &Path, exec_path: &Path, envs: &[String]) -> Result<(), String> {
    // The child env: only the explicit KEY=VALUE pairs. A blank env (no
    // PATH, no HOME) is deliberate — the daemon supplies everything the
    // instance needs, and the instance must not inherit daemon context.
    let env: Vec<CString> = envs
        .iter()
        .map(|pair| CString::new(pair.as_str().to_owned()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("env pair with NUL byte: {e}"))?;

    // chdir into the instance dir so a relative-path exec stays anchored and
    // core dumps land somewhere scoped.
    let dir_c = CString::new(instance_dir.as_os_str().as_bytes())
        .map_err(|e| format!("instance dir NUL byte: {e}"))?;
    let exec_c = CString::new(exec_path.as_os_str().as_bytes())
        .map_err(|e| format!("exec path NUL byte: {e}"))?;
    // argv[0] as the invoked path, no further args: the instance is
    // env-driven by design.
    let argv: Vec<CString> = vec![exec_c.clone()];

    // fork #1
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(format!("fork: {}", std::io::Error::last_os_error()));
    }
    if pid > 0 {
        // Parent: reap child #1 immediately (it exits after forking the
        // grandchild), then return success to the daemon.
        unsafe {
            libc::waitpid(pid, std::ptr::null_mut(), 0);
        }
        return Ok(());
    }

    // Child #1: new session, detach from the controlling terminal.
    if unsafe { libc::setsid() } < 0 {
        let e = std::io::Error::last_os_error();
        eprintln!("kallip-daemon-spawn: setsid: {e}");
        std::process::exit(71);
    }

    // fork #2 — the grandchild can never reacquire a controlling terminal
    // and is reparented to init when child #1 exits.
    let pid2 = unsafe { libc::fork() };
    if pid2 < 0 {
        let e = std::io::Error::last_os_error();
        eprintln!("kallip-daemon-spawn: fork2: {e}");
        std::process::exit(71);
    }
    if pid2 > 0 {
        std::process::exit(0); // child #1 done
    }

    // Grandchild: chdir, blank env + execve. Null-terminated argv/envp
    // pointer arrays (the CString values above outlive the call).
    let mut argv_p: Vec<*const libc::c_char> = argv.iter().map(|c| c.as_ptr()).collect();
    argv_p.push(std::ptr::null());
    let mut env_p: Vec<*const libc::c_char> = env.iter().map(|c| c.as_ptr()).collect();
    env_p.push(std::ptr::null());
    unsafe {
        libc::chdir(dir_c.as_ptr());
    }
    unsafe {
        libc::execve(exec_c.as_ptr(), argv_p.as_ptr(), env_p.as_ptr());
        // execve only returns on failure.
    }
    let e = std::io::Error::last_os_error();
    eprintln!("kallip-daemon-spawn: exec {exec_path:?}: {e}");
    std::process::exit(71);
}
