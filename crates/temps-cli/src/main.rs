//! Temps CLI - Single entrypoint for all services
//!
//! This binary delegates to `temps_cli::run` (defined in `lib.rs`) so the
//! same dispatch can be reused by EE-bundled binaries that need to
//! register additional plugins. See ADR 0001 §"Extension points exposed
//! by OSS".

fn main() -> anyhow::Result<()> {
    temps_cli::run(Vec::new())
}
