//! Keeping other programs of the same user out of this process's memory.

use rustix::process::{DumpableBehavior, set_dumpable_behavior};

use crate::PlatformError;

/// Marks the process as not dumpable (`PR_SET_DUMPABLE` 0): other
/// programs running as the same user can no longer attach a debugger to it
/// or read its memory through `/proc`, and it writes no core dumps. Root
/// still can. Programs it starts, such as the webview's own processes, are
/// dumpable again once they run.
///
/// # Errors
///
/// Fails when the kernel refuses, which it does not in practice.
pub fn deny_memory_access() -> Result<(), PlatformError> {
    set_dumpable_behavior(DumpableBehavior::NotDumpable).map_err(|e| PlatformError::Io(e.into()))
}

#[cfg(test)]
mod tests {
    use rustix::process::dumpable_behavior;

    use super::*;

    #[test]
    fn the_process_stops_being_dumpable() {
        deny_memory_access().unwrap();
        assert_eq!(dumpable_behavior().unwrap(), DumpableBehavior::NotDumpable);
    }
}
