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
    let arguments: Vec<String> = env::args().collect();
    if arguments.len() != 3 {
        return Err("usage: topology-smoke <mihomo.exe> <runtime-dir>".into());
    }
    let executable = PathBuf::from(&arguments[1]);
    let runtime = PathBuf::from(&arguments[2]);
    let verifier = MihomoManager::new(MihomoConfig::new(&executable, &runtime))?;
    verifier.verify_binary()?;
    fs::create_dir_all(&runtime)?;

    let controller_port = reserve_port()?;
    let first_port = reserve_port()?;
    let second_port = reserve_port()?;
    let first_id = Uuid::new_v4();
    let second_id = Uuid::new_v4();
    let secret = Uuid::new_v4().simple().to_string();
    let config = render_topology(
        &CoreTopology {
            slots: vec![
                CoreSlot {
                    id: first_id,
                    local_port: first_port,
                    selected: None,
                },
                CoreSlot {
                    id: second_id,
                    local_port: second_port,
                    selected: None,
                },
            ],
            available_nodes: vec![],
            download_port: None,
        },
        controller_port,
        &secret,
    )?;
    let config_path = runtime.join("topology.yaml");
    fs::write(&config_path, config)?;

    let mut command = Command::new(&executable);
    command
        .args(["-d"])
        .arg(&runtime)
        .args(["-f"])
        .arg(&config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn()?;
    let pid = child.id().ok_or("Mihomo PID unavailable")?;
    let controller = MihomoController::new(controller_port, secret)?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if Instant::now() >= deadline {
            child.start_kill()?;
            return Err("Controller did not become ready".into());
        }
        if controller
            .selected(&slot_selector_name(first_id))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let first_selector = slot_selector_name(first_id);
    let second_selector = slot_selector_name(second_id);
    controller.select(&first_selector, "DIRECT").await?;
    controller.select(&second_selector, "DIRECT").await?;
    if controller.selected(&first_selector).await? != "DIRECT"
        || controller.selected(&second_selector).await? != "DIRECT"
        || child.id() != Some(pid)
    {
        child.start_kill()?;
        return Err("independent hot selector switching failed".into());
    }
    controller.select(&first_selector, "REJECT").await?;
    if controller.selected(&first_selector).await? != "REJECT"
        || controller.selected(&second_selector).await? != "DIRECT"
        || child.id() != Some(pid)
    {
        child.start_kill()?;
        return Err("switching one Slot altered another Slot or restarted Core".into());
    }

    child.start_kill()?;
    child.wait().await?;
    TcpListener::bind(("127.0.0.1", first_port))?;
    TcpListener::bind(("127.0.0.1", second_port))?;
    println!(
        "result=PASS pid={pid} listeners={first_port},{second_port} first=REJECT second=DIRECT pid_unchanged=true ports_released=true"
    );
    Ok(())
}

fn reserve_port() -> std::io::Result<u16> {
    TcpListener::bind(("127.0.0.1", 0))?
        .local_addr()
        .map(|address| address.port())
}
