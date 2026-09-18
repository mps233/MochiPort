// The `mochiport` binary is intentionally a thin wrapper: the daemon, the CLI
// commands and the Codex desktop integration all live in the `mochiport`
// library, so the same implementation is reusable and testable in-process.

fn main() -> anyhow::Result<()> {
    mochiport::run()
}
