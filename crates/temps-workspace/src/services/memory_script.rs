//! Memory CLI script — re-exports from `temps-core` so this crate and the
//! agent executor share a single source of truth for the bash script.
//!
//! Both workspace chat sessions (this crate) and workflow runs
//! (`temps-agents::services::executor::AgentExecutor`) install the exact
//! same script in their sandboxes, just from different code paths.

pub use temps_core::{
    memory_install_command as install_command, MEMORY_SCRIPT, MEMORY_SCRIPT_DIR, MEMORY_SCRIPT_PATH,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_re_exports_match_temps_core() {
        // Sanity-check that the re-exports actually point at the temps-core
        // constants. If someone forks the constants, this test catches it.
        assert_eq!(MEMORY_SCRIPT_PATH, temps_core::MEMORY_SCRIPT_PATH);
        assert_eq!(MEMORY_SCRIPT_DIR, temps_core::MEMORY_SCRIPT_DIR);
        assert!(MEMORY_SCRIPT.contains("memory write"));
    }

    #[test]
    fn test_install_command_re_export_works() {
        let cmd = install_command();
        assert_eq!(cmd[0], "sh");
        assert_eq!(cmd[1], "-c");
        assert!(cmd[2].contains("/workspace/.temps/bin/memory"));
    }
}
