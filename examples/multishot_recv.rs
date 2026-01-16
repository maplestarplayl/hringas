use hringas::{buffer, IoUring};
use rustix::fd::AsFd;
use rustix::io::Errno;
use rustix::io_uring::{IoringCqeFlags, IORING_CQE_BUFFER_SHIFT};
use rustix::net::{self, AddressFamily, SendFlags, SocketFlags, SocketType};

fn main() {
    let mut ring = IoUring::new(4).unwrap();

    let (sock_rx, sock_tx) = net::socketpair(
        AddressFamily::UNIX,
        SocketType::DGRAM,
        SocketFlags::CLOEXEC,
        None,
    )
    .unwrap();

    let bgid = 7;
    let ring_entries = 8;
    let buf_ring = buffer::BufRing::new(bgid, ring_entries).unwrap();

    match unsafe { ring.register_pbuf_ring(&buf_ring) } {
        Ok(_) => {}
        Err(Errno::INVAL) => {
            eprintln!("provided buffer ring not supported (kernel < 5.19?)");
            return;
        }
        Err(e) => panic!("register_pbuf_ring failed: {e:?}"),
    }

    let mut buffers: Vec<Vec<u8>> =
        (0..ring_entries).map(|_| vec![0u8; 128]).collect();

    for (i, buf) in buffers.iter_mut().enumerate() {
        unsafe {
            buf_ring.push(buf.as_mut_ptr() as u64, buf.len() as u32, i as u16);
        }
    }
    buf_ring.sync();

    let sqe = ring.get_sqe().unwrap();
    sqe.prep_multishot_recv(0x2222_2222, sock_rx.as_fd(), bgid);
    unsafe {
        ring.submit().unwrap();
    }

    let messages = [b"one", b"two"];
    for msg in messages {
        net::send(sock_tx.as_fd(), msg, SendFlags::empty()).unwrap();
    }

    let mut received = 0;
    while received < messages.len() {
        let cqe = unsafe { ring.copy_cqe() }.unwrap();
        if cqe.user_data.u64_() != 0x2222_2222 {
            continue;
        }

        if cqe.res == -Errno::INVAL.raw_os_error() {
            eprintln!("multishot recv not supported (kernel < 6.0?)");
            return;
        }
        if cqe.res < 0 {
            eprintln!("recv failed: {cqe:?}");
            return;
        }

        let flags = cqe.flags.bits();
        let bid = (flags >> IORING_CQE_BUFFER_SHIFT) as usize;
        let len = cqe.res as usize;
        let data = &buffers[bid][..len];
        println!(
            "recv {} bytes (bid={}, more={})",
            len,
            bid,
            cqe.flags.contains(IoringCqeFlags::MORE)
        );
        println!("data: {:?}", String::from_utf8_lossy(data));

        unsafe {
            buf_ring.push(
                buffers[bid].as_mut_ptr() as u64,
                buffers[bid].len() as u32,
                bid as u16,
            );
        }
        buf_ring.sync();
        received += 1;
    }

    let _ = unsafe { ring.unregister_pbuf_ring(bgid) };
}
