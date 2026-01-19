use iou::ping;
use iou::router::Router;
use std::net::Ipv4Addr;
use std::time::Duration;

const BUF_SIZE: usize = 4096;
const RX_COUNT: usize = 256;
const TX_COUNT: usize = 256;

#[tokio::test]
async fn test_icmp_echo_reply() {
    let _ = tracing_subscriber::fmt::try_init();

    let local_ip = Ipv4Addr::new(10, 99, 99, 1);

    let router = Router::with_config("tun_ping", local_ip, 24, BUF_SIZE, RX_COUNT, TX_COUNT)
        .await
        .expect("Failed to start router");

    // Give reactor time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send ICMP Echo Request and verify we get a reply
    let rtt = ping::send(local_ip, 1).expect("Ping failed - no ICMP Echo Reply received");

    assert!(rtt < Duration::from_millis(100), "RTT too high: {:?}", rtt);

    router.shutdown().await.expect("Failed to shutdown router");
}

#[tokio::test]
async fn test_icmp_echo_burst() {
    let _ = tracing_subscriber::fmt::try_init();

    let local_ip = Ipv4Addr::new(10, 99, 98, 1);

    let router = Router::with_config("tun_burst", local_ip, 24, BUF_SIZE, RX_COUNT, TX_COUNT)
        .await
        .expect("Failed to start router");

    // Give reactor time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send burst of pings
    let mut success_count = 0;
    for seq in 0..100u16 {
        if ping::send(local_ip, seq).is_ok() {
            success_count += 1;
        }
    }

    assert_eq!(
        success_count, 100,
        "Expected 100 successful pings, got {}",
        success_count
    );

    router.shutdown().await.expect("Failed to shutdown router");
}
