use node2socks_core_adapter::{
    controller::MihomoController,
    mihomo::{MihomoConfig, MihomoManager},
    topology::{CoreSlot, CoreTopology, render_topology, slot_selector_name},
};
use std::{env, fs, net::TcpListener, path::PathBuf, process::Stdio, time::Duration};
use tokio::{process::Command, time::Instant};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 3 {
        return Err("usage: fail-closed-smoke <mihomo.exe> <runtime-dir>".into());
    }
    let executable = PathBuf::from(&args[1]);
    let runtime = PathBuf::from(&args[2]);
    MihomoManager::new(MihomoConfig::new(&executable, &runtime))?.verify_binary()?;
    fs::create_dir_all(&runtime)?;
    let controller_port = reserve()?;
    let slot_port = reserve()?;
    let slot = Uuid::new_v4();
    let secret = Uuid::new_v4().simple().to_string();
    let config = render_topology(
        &CoreTopology {
            slots: vec![CoreSlot {
                id: slot,
                local_port: slot_port,
                selected: None,
            }],
            available_nodes: vec![],
        },
        controller_port,
        &secret,
    )?;
    let path = runtime.join("fail-closed.yaml");
    fs::write(&path, config)?;
    let mut cmd = Command::new(&executable);
    cmd.args(["-d"])
        .arg(&runtime)
        .args(["-f"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn()?;
    let pid = child.id().ok_or("missing pid")?;
    let controller = MihomoController::new(controller_port, secret)?;
    let selector = slot_selector_name(slot);
    let deadline = Instant::now() + Duration::from_secs(15);
    while controller.selected(&selector).await.is_err() {
        if Instant::now() > deadline {
            child.start_kill()?;
            return Err("controller timeout".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    controller.select(&selector, "DIRECT").await?;
    controller.select(&selector, "REJECT").await?;
    if controller.selected(&selector).await? != "REJECT" || child.id() != Some(pid) {
        return Err("fail closed confirmation failed".into());
    }
    child.start_kill()?;
    child.wait().await?;
    TcpListener::bind(("127.0.0.1", slot_port))?;
    println!(
        "result=PASS pid={pid} slot={slot_port} target=REJECT pid_unchanged=true port_released=true"
    );
    Ok(())
}
fn reserve() -> std::io::Result<u16> {
    TcpListener::bind(("127.0.0.1", 0))?
        .local_addr()
        .map(|a| a.port())
}
