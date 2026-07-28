//! Demonstrates runtime configuration hot-reload (#633): a long-running
//! process can pick up edits to its config file without restarting.
//!
//! Run:
//!   cargo run --example config_hot_reload_example

use anchorkit::config::RuntimeConfigManager;

fn main() {
    let path = "configs/remittance-anchor.toml";

    // Load once at startup, just like a normal config load.
    let manager = RuntimeConfigManager::new(path)
        .unwrap_or_else(|e| panic!("failed to load initial config '{path}': {e}"));

    println!(
        "Loaded '{}' (network: {}, {} attestor(s))",
        manager.current().contract.name,
        manager.current().contract.network,
        manager.current().attestors.registry.len(),
    );

    // In a real long-running service, this would be a loop that runs for the
    // lifetime of the process (e.g. driven by a timer or a SIGHUP handler):
    //
    //   loop {
    //       std::thread::sleep(std::time::Duration::from_secs(5));
    //       match manager.reload_if_changed() {
    //           Ok(true)  => log::info!("config reloaded from {}", manager.path().display()),
    //           Ok(false) => {} // nothing changed on disk, no-op
    //           Err(e)    => log::warn!("config reload rejected, keeping previous config: {e}"),
    //       }
    //       // Downstream code always reads the current snapshot on demand:
    //       let cfg = manager.current();
    //       serve_with(&cfg);
    //   }
    //
    // A single explicit reload — e.g. triggered by an admin API call or a
    // file-watcher — looks like this:
    match manager.reload() {
        Ok(()) => println!("Explicit reload succeeded (no changes detected on disk)."),
        Err(e) => println!("Reload rejected — previous config kept active: {e}"),
    }

    // An invalid edit (e.g. an empty contract.name, or an operation template
    // referencing an unknown attestor) is rejected without disturbing the
    // configuration already in memory:
    println!(
        "Config still active after the reload attempt above: {}",
        manager.current().contract.name
    );
}
