use hringas::IoUring;
use rustix::fd::{AsFd, FromRawFd, OwnedFd};
use rustix::io::Errno;
use rustix::io_uring::IoringCqeFlags;
use rustix::net::{
    self, AddressFamily, SocketAddrUnix, SocketFlags, SocketType,
};
use tempfile::TempDir;

fn main() {
    let mut ring = IoUring::new(4).unwrap();

    let listener = net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )
    .unwrap();

    let tmp = TempDir::new().unwrap();
    let addr =
        SocketAddrUnix::new(tmp.path().join("hringas_ms_accept.sock")).unwrap();
    net::bind(listener.as_fd(), &addr).unwrap();
    net::listen(listener.as_fd(), 8).unwrap();

    let sqe = ring.get_sqe().unwrap();
    sqe.prep_multishot_accept(0x1111_1111, listener.as_fd());
    assert_eq!(unsafe { ring.submit() }, Ok(1));

    let client_a = net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )
    .unwrap();
    net::connect(client_a.as_fd(), &addr).unwrap();

    let client_b = net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )
    .unwrap();
    net::connect(client_b.as_fd(), &addr).unwrap();

    let mut accepted = 0;
    let mut saw_more = false;
    while accepted < 2 {
        let cqe = unsafe { ring.copy_cqe() }.unwrap();
        if cqe.user_data.u64_() != 0x1111_1111 {
            continue;
        }
        if cqe.res == -Errno::INVAL.raw_os_error() {
            eprintln!("multishot accept not supported (kernel < 5.19?)");
            return;
        }
        assert!(cqe.res >= 0, "accept failed: {cqe:?}");
        if cqe.flags.contains(IoringCqeFlags::MORE) {
            saw_more = true;
        }
        let _accepted_fd = unsafe { OwnedFd::from_raw_fd(cqe.res) };
        accepted += 1;
    }

    assert!(saw_more, "expected CQE_F_MORE for multishot accept");
}
