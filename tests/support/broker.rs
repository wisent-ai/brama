//! A real Skarbiec service owned by the test, never by Brama's launcher.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

use crate::support::SkarbiecVault;

pub(super) struct SharedBroker {
    child: Child,
    socket: PathBuf,
}

impl SharedBroker {
    pub(super) fn start(vault: &SkarbiecVault) -> Self {
        // Reuse the existing short GnuPG fixture root: macOS caps Unix paths at 104 bytes.
        let socket = vault.root.join("broker.sock");
        let mut command = Command::new(vault.router());
        vault.apply_environment(&mut command);
        let child = command
            .args(["serve", "--port", "0", "--socket"])
            .arg(&socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the real shared Skarbiec service");
        let mut broker = Self { child, socket };
        let stderr = broker.child.stderr.take().expect("broker stderr");
        let socket_text = broker.socket.to_string_lossy().into_owned();
        let (announced, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut announced = Some(announced);
            let mut startup_log = String::new();
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else {
                    break;
                };
                eprintln!("[shared Skarbiec] {line}");
                if announced.is_some() {
                    startup_log.push_str(&line);
                    startup_log.push('\n');
                }
                // This only orders the request below; the log is not readiness.
                if line.contains(&socket_text) {
                    if let Some(announced) = announced.take() {
                        let _ = announced.send(Ok(()));
                    }
                }
            }
            if let Some(announced) = announced {
                let _ = announced.send(Err(startup_log));
            }
        });
        match receiver.recv() {
            Ok(Ok(())) => {}
            Ok(Err(log)) => panic!("shared broker ended before reporting its endpoint: {log}"),
            Err(error) => panic!("shared broker log reader stopped: {error}"),
        }
        let status = vault.skarbiec(&[
            "capability-status",
            "--socket",
            broker.socket.to_str().expect("socket path UTF-8"),
        ]);
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
        broker
    }

    pub(super) fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Drop for SharedBroker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
