use node2socks_core_adapter::{
    ProxyCore,
    mihomo::{MihomoConfig, MihomoManager},
};
use std::{env, path::PathBuf, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = env::args().collect();
    if arguments.len() != 3 {
        return Err("usage: core-recovery-smoke <mihomo.exe> <runtime-dir>".into());
    }
    let manager = MihomoManager::new(MihomoConfig {
        executable: PathBuf::from(&arguments[1]),
        runtime_dir: PathBuf::from(&arguments[2]),
        socks_port: None,
        topology: None,
        providers: Vec::new(),
        startup_timeout: Duration::from_secs(15),
        shutdown_timeout: Duration::from_secs(5),
        max_restart_attempts: 3,
        outbound_interface: None,
    })?;

    let first = manager.start().await?;
    let first_pid = first.pid.ok_or("initial PID unavailable")?;
    force_terminate(first_pid)?;
    tokio::time::sleep(Duration::from_millis(250)).await;

    let recovered = manager
        .recover_if_crashed()
        .await?
        .ok_or("manager did not detect the forced crash")?;
    let recovered_pid = recovered.pid.ok_or("recovered PID unavailable")?;
    if first_pid == recovered_pid {
        return Err("recovery reused the terminated PID unexpectedly".into());
    }
    manager.stop().await?;
    if process_exists(recovered_pid)? {
        return Err(format!("recovered Mihomo PID {recovered_pid} remained alive").into());
    }
    println!(
        "result=PASS crash_pid={first_pid} recovered_pid={recovered_pid} state={:?} process_gone=true",
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
