use node2socks_core_adapter::{
    ProxyCore,
    mihomo::{MIHOMO_EXECUTABLE_SHA256, MIHOMO_VERSION, MihomoConfig, MihomoManager},
};
use std::{env, net::TcpListener, path::PathBuf, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("node2socks_core_adapter=info,mihomo=info")
        .with_target(false)
        .init();

    let arguments: Vec<String> = env::args().collect();
    if arguments.len() < 3 {
        return Err("usage: core-smoke <mihomo.exe> <runtime-dir> [cycles]".into());
    }
    let executable = PathBuf::from(&arguments[1]);
    let runtime_root = PathBuf::from(&arguments[2]);
    let cycles = arguments
        .get(3)
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(1);

    let mut passed = 0_u32;
    for cycle in 1..=cycles {
        let runtime_dir = runtime_root.join(format!("cycle-{cycle}"));
        let manager = MihomoManager::new(MihomoConfig {
            executable: executable.clone(),
            runtime_dir,
            socks_port: None,
            topology: None,
            providers: Vec::new(),
            startup_timeout: Duration::from_secs(15),
            shutdown_timeout: Duration::from_secs(5),
            outbound_interface: None,
            max_restart_attempts: 3,
        })?;
        let checksum = manager.verify_binary()?;
        let health = manager.start().await?;
        let socks_port = manager.socks_port().await.ok_or("SOCKS port unavailable")?;
        let pid = health.pid.ok_or("PID unavailable")?;
        println!(
            "cycle={cycle} phase=start pid={pid} socks=127.0.0.1:{socks_port} controller={} version={}",
            health.controller_address.as_deref().unwrap_or("unknown"),
            health.version.as_deref().unwrap_or("unknown")
        );
        manager.stop().await?;

        TcpListener::bind(("127.0.0.1", socks_port)).map_err(|error| {
            format!("cycle {cycle}: SOCKS port {socks_port} remained occupied: {error}")
        })?;
        if process_exists(pid)? {
            return Err(format!("cycle {cycle}: Mihomo PID {pid} remained alive").into());
        }
        println!("cycle={cycle} phase=stop pid={pid} ports_released=true process_gone=true");
        if checksum != MIHOMO_EXECUTABLE_SHA256 {
            return Err("verified executable checksum changed unexpectedly".into());
        }
        passed += 1;
    }

    println!(
        "result=PASS cycles={passed}/{cycles} mihomo={MIHOMO_VERSION} executable_sha256={MIHOMO_EXECUTABLE_SHA256}"
    );
    Ok(())
}

#[cfg(windows)]
fn process_exists(pid: u32) -> Result<bool, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("tasklist.exe")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.contains(&pid.to_string()))
}

#[cfg(not(windows))]
fn process_exists(_pid: u32) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(false)
}
