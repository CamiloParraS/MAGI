//! One running app per user (SPEC.md §6.3 `--toggle`): the first process
//! listens on a local socket (a named pipe on Windows, a Unix socket
//! elsewhere; never TCP, NFR-9); later ones forward their request and exit.

use std::io::{self, BufRead, BufReader, Write};

use interprocess::local_socket::{
    GenericNamespaced, Listener, ListenerOptions, Stream, prelude::*,
};

pub enum Claim {
    Primary(Listener),
    Secondary,
}

/// Per user: Windows pipe names are machine-wide.
pub fn socket_name() -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    format!("magi-{user}.sock")
}

fn listen(name: &str, overwrite: bool) -> io::Result<Listener> {
    ListenerOptions::new()
        .name(name.to_ns_name::<GenericNamespaced>()?)
        .try_overwrite(overwrite)
        .create_sync()
}

/// Becomes the listener, or finds one already running. A name that is taken
/// but answers nobody is a stale socket file, left by a crash or a kill on
/// macOS (Unix sockets there are files in `/tmp`): it is replaced, or no later
/// launch could ever be single-instance or take `--toggle`. When even that
/// fails the bind error is returned and the caller runs without
/// single-instance rather than not at all.
pub fn claim(name: &str) -> io::Result<Claim> {
    match listen(name, false) {
        Ok(listener) => Ok(Claim::Primary(listener)),
        Err(bind) => match Stream::connect(name.to_ns_name::<GenericNamespaced>()?) {
            Ok(_) => Ok(Claim::Secondary),
            Err(_) => listen(name, true).map(Claim::Primary).map_err(|_| bind),
        },
    }
}

pub fn forward(name: &str, message: &str) -> io::Result<()> {
    let mut stream = Stream::connect(name.to_ns_name::<GenericNamespaced>()?)?;
    stream.write_all(format!("{message}\n").as_bytes())
}

/// Calls `on_message` with each non-empty line, on a thread of its own.
/// `claim`'s probe connection sends nothing and is ignored.
pub fn serve(listener: Listener, on_message: impl Fn(String) + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("magi-instance".into())
        .spawn(move || {
            for conn in listener.incoming().filter_map(Result::ok) {
                let mut line = String::new();
                if BufReader::new(conn).read_line(&mut line).is_ok() {
                    let line = line.trim();
                    if !line.is_empty() {
                        on_message(line.to_owned());
                    }
                }
            }
        });
    if let Err(error) = spawned {
        tracing::error!(%error, "could not listen for other launches");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_second_launch_forwards_its_message_to_the_first() {
        let name = format!("magi-test-{}.sock", std::process::id());
        let Claim::Primary(listener) = claim(&name).unwrap() else {
            panic!("the first claim must be primary");
        };
        let (tx, rx) = std::sync::mpsc::channel();
        serve(listener, move |m| {
            let _ = tx.send(m);
        });
        assert!(matches!(claim(&name).unwrap(), Claim::Secondary));
        forward(&name, "toggle").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "toggle");
    }

    /// macOS has no abstract socket namespace: the name is a file in `/tmp`,
    /// and a process that dies without cleanup leaves it behind. (Not run on
    /// Linux, whose namespaced sockets vanish with their process.)
    #[cfg(target_os = "macos")]
    #[test]
    fn a_socket_file_left_by_a_crash_does_not_end_single_instance() {
        let name = format!("magi-stale-{}.sock", std::process::id());
        let path = std::path::PathBuf::from("/tmp").join(&name);
        // std's listener does not unlink on drop: this is the crash.
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        assert!(matches!(claim(&name).unwrap(), Claim::Primary(_)));
        let _ = std::fs::remove_file(path);
    }
}
