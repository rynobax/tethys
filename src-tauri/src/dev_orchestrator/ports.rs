//! Free-port detection. We bind a TCP listener to `0.0.0.0:N` to
//! test — if `bind` succeeds we know nothing else has it on **any
//! interface**, then we drop the listener so the spawning process can
//! grab it. There's a small TOCTOU window between drop and re-bind;
//! in practice that's fine for a dev workflow where the next bind
//! happens within ms.
//!
//! We use `0.0.0.0` rather than `127.0.0.1` because tools like
//! rspack-serve bind to `[::]:N` (IPv6 wildcard) by default — on
//! macOS that wildcard captures the IPv4 port too, so an IPv4-only
//! probe of `127.0.0.1:N` would falsely report the port as free while
//! `*:N` is taken. `0.0.0.0` binding fails when anything has the
//! port on either stack, which matches what yarn dev will hit.

use std::net::TcpListener;

/// First free port >= `start`. Returns `None` if no port in
/// [start, start+200) is free (effectively never, in our usage).
pub fn find_free_port_from(start: u16) -> Option<u16> {
    for port in start..start.saturating_add(200) {
        if TcpListener::bind(("0.0.0.0", port)).is_ok() {
            return Some(port);
        }
    }
    None
}

/// `true` if anything is listening on `port` (any interface). Returns
/// `false` on the binding-error path (port is bound = our test bind fails).
pub fn is_port_bound(port: u16) -> bool {
    TcpListener::bind(("0.0.0.0", port)).is_err()
}
