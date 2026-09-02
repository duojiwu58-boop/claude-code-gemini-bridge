// Computer Use execution lives in `bin/claude-computer-host.rs`.
//
// This zero-sized marker keeps older AppState construction helpers source-compatible while the
// service-side Host broker, polling endpoints, and GUI approval API remain intentionally removed.
#[derive(Default)]
struct ComputerBroker;
