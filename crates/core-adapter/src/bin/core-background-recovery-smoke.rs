use node2socks_core_adapter::{
    ProxyCore,
    mihomo::{MihomoConfig, MihomoManager},
};
use std::{env, path::PathBuf, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = env::args().collect();
    if arguments.len() != 3 {
        return Err("usage: core-background-recovery-smoke <mihomo.exe> <runtime-dir>".into());
    }
    let manager = Arc::new(MihomoManager::new(MihomoConfig {
        executable: PathBuf::from(&arguments[1]),
        runtime_dir: PathBuf::from(&arguments[2]),
        socks_port: None,
        topology: None,
        providers: Vec::new(),
        startup_timeout: Duration::from_secs(15),
        shutdown_timeout: Duration::from_secs(5),
        max_restart_attempts: 3,
        outbound_interface: None,
    })?);

    let first = manager.start().await?;
    let first_pid = first.pid.ok_or("initial PID unavailable")?;
    let monitor = manager
        .clone()
        .spawn_crash_monitor(Duration::from_millis(100));
    force_terminate(first_pid)?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let recovered = loop {
        if tokio::time::Instant::now() >= deadline {
            return Err("background monitor did not recover Mihomo before timeout".into());
        }
        if let Ok(health) = manager.health().await
            && health.pid.is_some_and(|pid| pid != first_pid)
        {
            break health;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let recovered_pid = recovered.pid.ok_or("recovered PID unavailable")?;
    monitor.shutdown().await;
    manager.stop().await?;
    if process_exists(recovered_pid)? {
        return Err(format!("recovered Mihomo PID {recovered_pid} remained alive").into());
    }
    println!(
        "result=PASS mode=background-monitor crash_pid={first_pid} recovered_pid={recovered_pid} state={:?} process_gone=true",
        recovered.state
    );
    Ok(())
}

#[cfg(windows)]
fn force_terminate(pid: u32) -> Result<(), Box<dyn std::error::Error>> {
    let status = std::process::Command::new("taskkill.exe")
        .args(["/PID", &pid.to_string(), "/F"])
        .status()?;
    if !status.success() {
        return Err(format!("taskkill failed for PID {pid}: {status}").into());
    }
    Ok(())
}

#[cfg(windows)]
fn process_exists(pid: u32) -> Result<bool, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("tasklist.exe")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
}

#[cfg(not(windows))]
fn force_terminate(_pid: u32) -> Result<(), Box<dyn std::error::Error>> {
    Err("crash injection is only implemented on Windows".into())
}

#[cfg(not(windows))]
fn process_exists(_pid: u32) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(false)
}
