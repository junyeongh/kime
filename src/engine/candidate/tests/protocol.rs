//! Exercises the parent side of the candidate window protocol against a stub
//! child, so the wire format is covered without needing a GUI or a display.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process;
use std::sync::{Mutex, MutexGuard, OnceLock};

use kime_engine_candidate::client::{Client, CANDIDATE_PROCESS_NAME};

const CANDIDATES: &[(&str, &str)] = &[("漢", "한수 한"), ("韓", "나라 한"), ("恨", "한 한")];

/// `PATH` is process wide, so these tests must not overlap. Without this a test
/// can spawn another test's stub and block forever waiting for a reply that stub
/// will never send.
fn path_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Install a stub `kime-candidate-window` on `PATH` and return its directory.
///
/// Every stub consumes the candidate list up to the `end` marker first, which is
/// what proves the list framing no longer depends on stdin being closed.
fn install_stub(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kime-candidate-test-{}-{name}", process::id()));
    fs::create_dir_all(&dir).unwrap();

    let script = format!(
        "#!/bin/sh\n\
         while IFS= read -r line; do\n\
         \t[ \"$line\" = end ] && break\n\
         done\n\
         {body}\n"
    );

    let path = dir.join(CANDIDATE_PROCESS_NAME);
    fs::File::create(&path)
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();

    dir
}

/// Point `PATH` at a stub for the duration of `f`.
fn with_stub<T>(name: &str, body: &str, f: impl FnOnce() -> T) -> T {
    let _guard = path_lock();

    let dir = install_stub(name, body);
    let old = std::env::var_os("PATH");

    std::env::set_var("PATH", &dir);
    let out = f();

    match old {
        Some(path) => std::env::set_var("PATH", path),
        None => std::env::remove_var("PATH"),
    }

    let _ = fs::remove_dir_all(&dir);
    out
}

#[test]
fn reports_the_selected_index() {
    with_stub("select", "echo 2", || {
        let client = Client::new(CANDIDATES).unwrap();
        while !client.is_ready() {}
        assert_eq!(client.close().unwrap(), Some(2));
    });
}

#[test]
fn empty_reply_is_a_cancellation() {
    with_stub("cancel", "exit 0", || {
        let client = Client::new(CANDIDATES).unwrap();
        while !client.is_ready() {}
        // Not an error, and crucially not a selection either.
        assert_eq!(client.close().unwrap(), None);
    });
}

#[test]
fn a_crashed_child_is_an_error_not_an_empty_selection() {
    with_stub("crash", "exit 1", || {
        let client = Client::new(CANDIDATES).unwrap();
        while !client.is_ready() {}
        // This used to come back as Ok(Some("")) and commit an empty string.
        assert!(client.close().is_err());
    });
}

#[test]
fn a_nonsense_reply_is_an_error() {
    with_stub("garbage", "echo not-an-index", || {
        let client = Client::new(CANDIDATES).unwrap();
        while !client.is_ready() {}
        assert!(client.close().is_err());
    });
}

#[test]
fn commands_reach_the_child_after_the_list() {
    // Echo back a distinct index depending on the command actually received,
    // which only works if stdin stayed open past the candidate list.
    let body = "IFS= read -r cmd\n\
                if [ \"$cmd\" = \"page 1\" ]; then echo 1; else echo 0; fi";

    with_stub("command", body, || {
        let mut client = Client::new(CANDIDATES).unwrap();
        client.set_page(1).unwrap();
        while !client.is_ready() {}
        assert_eq!(client.close().unwrap(), Some(1));
    });
}

#[test]
fn spawn_failure_is_reported() {
    let _guard = path_lock();

    let dir = std::env::temp_dir().join(format!("kime-candidate-test-{}-missing", process::id()));
    fs::create_dir_all(&dir).unwrap();

    let old = std::env::var_os("PATH");
    std::env::set_var("PATH", &dir);
    let result = Client::new(CANDIDATES);
    match old {
        Some(path) => std::env::set_var("PATH", path),
        None => std::env::remove_var("PATH"),
    }
    let _ = fs::remove_dir_all(&dir);

    assert!(result.is_err(), "missing binary must surface as an error");
}
