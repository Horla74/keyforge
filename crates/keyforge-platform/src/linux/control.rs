//! Per-UID abstract socket: no stale files, and peer credentials authorize control.
use crate::{Error, Result};
use std::io::{self, Read, Write};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::time::Duration;

fn address() -> io::Result<SocketAddr> {
    SocketAddr::from_abstract_name(format!("keyforge-{}", unsafe { libc::geteuid() }))
}
fn credentials(stream: &UnixStream) -> io::Result<libc::ucred> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of_val(&cred) as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if cred.uid != unsafe { libc::geteuid() } {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "control peer has a different UID",
        ));
    }
    Ok(cred)
}
pub struct Control(UnixListener);
impl Control {
    pub fn bind() -> io::Result<Self> {
        Self::bind_at(&address()?)
    }
    fn bind_at(addr: &SocketAddr) -> io::Result<Self> {
        let listener = UnixListener::bind_addr(addr)?;
        listener.set_nonblocking(true)?;
        Ok(Self(listener))
    }
    pub fn stop_requested(&self) -> io::Result<bool> {
        // Bound work per tick so probing cannot starve keyboard processing.
        for _ in 0..8 {
            match self.0.accept() {
                Ok((mut stream, _)) => {
                    if credentials(&stream).is_err() {
                        continue;
                    }
                    stream.set_read_timeout(Some(Duration::from_millis(10)))?;
                    let mut byte = [0];
                    if stream.read(&mut byte).ok() == Some(1) && byte[0] == b'S' {
                        return Ok(true);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        Ok(false)
    }
}
pub fn instance_running() -> bool {
    address()
        .and_then(|a| UnixStream::connect_addr(&a))
        .and_then(|s| credentials(&s))
        .is_ok()
}
pub fn signal_stop() -> Result<bool> {
    match address().and_then(|a| UnixStream::connect_addr(&a)) {
        Ok(mut stream) => {
            credentials(&stream).map_err(|e| Error::new(e.to_string()))?;
            stream
                .write_all(b"S")
                .map_err(|e| Error::new(e.to_string()))?;
            Ok(true)
        }
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            Ok(false)
        }
        Err(e) => Err(Error::new(e.to_string())),
    }
}
pub fn force_stop() -> Result<()> {
    let stream = address()
        .and_then(|a| UnixStream::connect_addr(&a))
        .map_err(|e| Error::new(e.to_string()))?;
    let cred = credentials(&stream).map_err(|e| Error::new(e.to_string()))?;
    if cred.pid <= 1 || cred.pid == std::process::id() as i32 {
        return Err(Error::new("invalid daemon PID"));
    }
    if unsafe { libc::kill(cred.pid, libc::SIGKILL) } != 0 {
        return Err(Error::new(io::Error::last_os_error().to_string()));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires permission to bind Linux abstract Unix sockets (outside restricted sandboxes)"]
    fn control_is_exclusive_and_probes_do_not_stop() {
        let addr = SocketAddr::from_abstract_name(format!("keyforge-test-{}", std::process::id()))
            .unwrap();
        let control = Control::bind_at(&addr).unwrap();
        assert!(Control::bind_at(&addr).is_err());
        drop(UnixStream::connect_addr(&addr).unwrap());
        assert!(!control.stop_requested().unwrap());
        let mut client = UnixStream::connect_addr(&addr).unwrap();
        client.write_all(b"S").unwrap();
        assert!(control.stop_requested().unwrap());
        drop(client);
        drop(control);
        assert!(Control::bind_at(&addr).is_ok());
    }
}
