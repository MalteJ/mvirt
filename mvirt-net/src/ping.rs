use smoltcp::wire::{Icmpv4Message, Icmpv4Packet, Icmpv4Repr};
use socket2::{Domain, Protocol, Socket, Type};
use std::io;
use std::mem::MaybeUninit;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, Instant};

pub fn send(target: Ipv4Addr, seq: u16) -> io::Result<Duration> {
    send_from(None, target, seq)
}

pub fn send_from(source: Option<Ipv4Addr>, target: Ipv4Addr, seq: u16) -> io::Result<Duration> {
    let socket = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4))?;
    socket.set_read_timeout(Some(Duration::from_secs(1)))?;

    if let Some(src) = source {
        let bind_addr = SocketAddrV4::new(src, 0);
        socket.bind(&bind_addr.into())?;
    }

    let ident = std::process::id() as u16;

    let icmp_repr = Icmpv4Repr::EchoRequest {
        ident,
        seq_no: seq,
        data: b"hello",
    };

    let mut buf = vec![0u8; icmp_repr.buffer_len()];
    let mut packet = Icmpv4Packet::new_unchecked(&mut buf);
    icmp_repr.emit(&mut packet, &smoltcp::phy::ChecksumCapabilities::default());

    let addr = SocketAddrV4::new(target, 0);
    let send_time = Instant::now();
    socket.send_to(&buf, &addr.into())?;

    let mut recv_buf = [MaybeUninit::<u8>::uninit(); 1500];

    loop {
        let (n, _) = socket.recv_from(&mut recv_buf)?;

        let buf_init: &[u8] =
            unsafe { std::slice::from_raw_parts(recv_buf.as_ptr() as *const u8, n) };

        if n < 20 {
            continue;
        }
        let ip_header_len = ((buf_init[0] & 0x0f) as usize) * 4;
        if n < ip_header_len + 8 {
            continue;
        }
        let icmp_data = &buf_init[ip_header_len..];

        let Ok(icmp) = Icmpv4Packet::new_checked(icmp_data) else {
            continue;
        };

        if icmp.msg_type() == Icmpv4Message::EchoReply
            && icmp.echo_ident() == ident
            && icmp.echo_seq_no() == seq
        {
            return Ok(send_time.elapsed());
        }
    }
}
