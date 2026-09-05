use super::*;

/// Return the live recorder PID/start pair observed through socket credentials.
pub fn recorder_identity(session: &Path) -> Option<(u32, u64)> {
    let manifest = read_manifest(session).ok()?;
    let address = SocketAddr::from_abstract_name(manifest.control_socket.as_bytes()).ok()?;
    let stream = UnixStream::connect_addr(&address).ok()?;
    let credentials = getsockopt(&stream, PeerCredentials).ok()?;
    let pid = u32::try_from(credentials.pid()).ok()?;
    return Some((pid, process_start_ticks(pid)?));
}

fn process_start_ticks(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    return stat
        .get(close + 1..)?
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok();
}
