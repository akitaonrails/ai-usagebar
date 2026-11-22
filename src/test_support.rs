//! Helpers shared by Unix subprocess tests.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// The argument a stand-in answers before running its body.
const PROBE: &str = "--test-probe";

/// Write `body` as an executable `#!/bin/sh` script at `path`, and return
/// once it can be executed.
///
/// Linux refuses to execute a file that any process holds open for writing
/// (`ETXTBSY`), and a sibling test that spawns while this one writes
/// inherits the descriptor until its own child execs. So the script first
/// answers a probe with `exit 0`, and the probe is retried until it runs:
/// once one exec succeeds, nothing can hold the file open for writing any
/// more, and the code under test never races that window.
pub(crate) fn write_script(path: &Path, body: &str) {
    std::fs::write(
        path,
        format!("#!/bin/sh\n[ \"$1\" = {PROBE} ] && exit 0\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..500 {
        match std::process::Command::new(path).arg(PROBE).status() {
            Ok(_) => return,
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => panic!("{}: {error}", path.display()),
        }
    }
    panic!("{} stayed busy", path.display());
}
