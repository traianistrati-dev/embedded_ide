//! Outgoing HTTP: a ureq agent whose timeout really is the timeout.
//!
//! ureq 2 bounds a request with `.timeout()` everywhere EXCEPT the two steps
//! that stall when the network is only half up. Connecting runs on the agent's
//! own `timeout_connect` - 30 s by default, and it takes precedence over
//! `.timeout()` - and DNS is not bounded at all. So the Cargo.toml feature
//! check, which gives itself 4 s on the UI THREAD during an import, froze the
//! window for 21-30 s on a host that never answered, and for as long as
//! Windows kept retrying a DNS server that was not there.

use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::time::Duration;

/// Resolving and connecting never get longer than this, however long the
/// whole request may take: ureq's own connect default, which the 10-minute AI
/// calls have always had.
const CONNECT_CAP: Duration = Duration::from_secs(30);

/// An agent for requests that end within `timeout`.
///
/// `timeout` bounds the whole request, as `.timeout()` did. DNS and the
/// connect now share one limit inside it - `timeout`, or [`CONNECT_CAP`] if
/// that is shorter - because ureq starts the connect clock before the lookup.
/// Only a redirect restarts that clock, so a hop begun just before the
/// deadline can overrun it, by at most that limit.
///
/// Cheap to build, so build one per request.
pub(crate) fn agent(timeout: Duration) -> ureq::Agent {
    agent_with(timeout, |netloc| {
        netloc.to_socket_addrs().map(Iterator::collect)
    })
}

/// [`agent`] with the system resolver swapped out - the tests' way to make
/// DNS hang on purpose.
fn agent_with(timeout: Duration, resolve: fn(&str) -> io::Result<Vec<SocketAddr>>) -> ureq::Agent {
    let step = timeout.min(CONNECT_CAP);
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .timeout_connect(step)
        .resolver(move |netloc: &str| resolve_within(netloc, step, resolve))
        .build()
}

/// `resolve(netloc)`, given up after `limit`.
///
/// The lookup itself cannot be interrupted, so it runs on a thread of its own
/// and is simply not waited for past `limit`; it ends when the system
/// resolver does. An IP literal needs no lookup and gets no thread.
fn resolve_within(
    netloc: &str,
    limit: Duration,
    resolve: fn(&str) -> io::Result<Vec<SocketAddr>>,
) -> io::Result<Vec<SocketAddr>> {
    if let Ok(addr) = netloc.parse::<SocketAddr>() {
        return Ok(vec![addr]);
    }
    let (tx, rx) = mpsc::channel();
    let owned = netloc.to_owned();
    std::thread::Builder::new()
        .name("dns lookup".into())
        .spawn(move || {
            // The receiver is gone once the caller stopped waiting.
            let _ = tx.send(resolve(&owned));
        })?;
    match rx.recv_timeout(limit) {
        Ok(answer) => answer,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("no DNS answer within {} s", limit.as_secs_f32()),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(io::Error::other("the DNS lookup ended without an answer"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const BUDGET: Duration = Duration::from_millis(400);
    /// Scheduling slack on a loaded test machine; the bugs these tests catch
    /// took 5 s (DNS, below) and 21 s (connect).
    const SLACK: Duration = Duration::from_millis(1600);

    /// `agent.get(url).call()`, which must FAIL within the budget.
    ///
    /// Run on a thread of its own and waited for only that long, so a lost
    /// limit fails here, with this message, instead of hanging the test run.
    fn call_failing_in_time(what: &str, agent: ureq::Agent, url: &str) -> ureq::Error {
        let (tx, rx) = mpsc::channel();
        let owned = url.to_owned();
        let start = Instant::now();
        std::thread::spawn(move || {
            let _ = tx.send(agent.get(&owned).call());
        });
        match rx.recv_timeout(BUDGET + SLACK) {
            Ok(Ok(_)) => panic!("{what}: nothing should answer {url}"),
            Ok(Err(e)) => e,
            Err(_) => panic!(
                "{what}: still waiting after {:?}, the budget was {BUDGET:?}",
                start.elapsed()
            ),
        }
    }

    /// The kind of the io error under a transport error.
    fn io_kind(e: &ureq::Error) -> Option<io::ErrorKind> {
        std::error::Error::source(e)?
            .downcast_ref::<io::Error>()
            .map(io::Error::kind)
    }

    /// The freeze in the import: a host that swallows the connection attempt.
    /// 192.0.2.1 is TEST-NET-1, reserved for documentation and never routed,
    /// so the SYN goes out and nothing ever comes back. On a machine with no
    /// network at all it fails at once, which passes just the same - the
    /// connect limit itself is pinned by the config test below.
    #[test]
    fn a_host_that_never_answers_ends_within_the_budget() {
        call_failing_in_time("connect", agent(BUDGET), "http://192.0.2.1:81/");
    }

    #[test]
    fn a_dns_server_that_never_answers_ends_within_the_budget() {
        fn hangs(_: &str) -> io::Result<Vec<SocketAddr>> {
            std::thread::sleep(Duration::from_secs(5));
            Err(io::Error::other("too late"))
        }
        let e = call_failing_in_time(
            "dns",
            agent_with(BUDGET, hangs),
            "http://no-answer.invalid/",
        );
        // Given up on, not answered: without the bounded resolver the system
        // one answers `.invalid` with a fast NXDOMAIN, which also "fails in
        // time".
        assert_eq!(e.kind(), ureq::ErrorKind::Dns, "{e}");
        assert_eq!(io_kind(&e), Some(io::ErrorKind::TimedOut), "{e}");
    }

    /// Connected, then silence: bounded by `.timeout()` before this module
    /// too, and it has to stay that way.
    #[test]
    fn a_server_that_never_replies_ends_within_the_budget() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let held = std::thread::spawn(move || listener.accept().map(|(s, _)| s));
        let e = call_failing_in_time("read", agent(BUDGET), &format!("http://127.0.0.1:{port}/"));
        // A read that timed out, not a reset: Windows says TimedOut, the
        // unixes WouldBlock.
        assert!(
            matches!(
                io_kind(&e),
                Some(io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
            ),
            "{e}"
        );
        drop(held);
    }

    /// A long request keeps ureq's 30 s connect, it does not get to wait ten
    /// minutes for a connection; a short one connects within its own budget.
    #[test]
    fn the_connect_limit_follows_the_budget_up_to_ureqs_default() {
        let config = |t| format!("{:?}", agent(t));
        let ai = config(Duration::from_secs(600));
        assert!(ai.contains("timeout_connect: Some(30s)"), "{ai}");
        assert!(ai.contains("timeout: Some(600s)"), "{ai}");
        let import = config(Duration::from_secs(4));
        assert!(import.contains("timeout_connect: Some(4s)"), "{import}");
        assert!(import.contains("timeout: Some(4s)"), "{import}");
    }

    #[test]
    fn a_resolved_name_and_an_ip_literal_come_back_whole() {
        fn two(_: &str) -> io::Result<Vec<SocketAddr>> {
            Ok(vec![
                "10.0.0.1:443".parse().unwrap(),
                "[::1]:443".parse().unwrap(),
            ])
        }
        let answer = resolve_within("example.test:443", BUDGET, two).unwrap();
        assert_eq!(answer.len(), 2);
        fn unreachable(_: &str) -> io::Result<Vec<SocketAddr>> {
            panic!("an IP literal must not be looked up")
        }
        let literal = resolve_within("127.0.0.1:80", BUDGET, unreachable).unwrap();
        assert_eq!(literal, vec!["127.0.0.1:80".parse().unwrap()]);
    }

    /// Every request goes through [`agent`]: `ureq::get` and friends use a
    /// default agent, whose connect waits 30 s whatever `.timeout()` says.
    #[test]
    fn every_request_is_built_on_this_agent() {
        /// Every ureq 2 entry point that sends through a default agent.
        const DIRECT: [&str; 12] = [
            "ureq::get(",
            "ureq::post(",
            "ureq::put(",
            "ureq::patch(",
            "ureq::delete(",
            "ureq::head(",
            "ureq::request(",
            "ureq::request_url(",
            "ureq::agent(",
            "ureq::builder(",
            "ureq::Agent::new(",
            "ureq::AgentBuilder",
        ];
        /// What `use ureq::…` may bring in: types to match on, nothing that
        /// sends, so no bare `get(url)` or `Agent::new()` slips past.
        const IMPORTABLE: [&str; 4] = ["Error", "ErrorKind", "Response", "Transport"];
        fn imports_a_sender(code: &str) -> bool {
            code.strip_prefix("use ureq::").is_some_and(|rest| {
                rest.trim_end_matches(';')
                    .trim_matches(|c| c == '{' || c == '}')
                    .split(',')
                    .map(str::trim)
                    .any(|name| !name.is_empty() && !IMPORTABLE.contains(&name))
            })
        }
        fn scan(dir: &std::path::Path, exempt: &std::path::Path, found: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    scan(&p, exempt, found);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs") || p == exempt {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                for (i, line) in text.lines().enumerate() {
                    let code = line.trim_start();
                    if !code.starts_with("//")
                        && (DIRECT.iter().any(|d| line.contains(d)) || imports_a_sender(code))
                    {
                        found.push(format!("{}:{}", p.display(), i + 1));
                    }
                }
            }
        }
        assert!(imports_a_sender("use ureq::get;"));
        assert!(imports_a_sender("use ureq::{Agent, Error};"));
        assert!(!imports_a_sender("use ureq::{Error, ErrorKind};"));
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        scan(&src, &src.join("net.rs"), &mut found);
        assert!(
            found.is_empty(),
            "build these requests on crate::net::agent:\n{}",
            found.join("\n")
        );
    }
}
