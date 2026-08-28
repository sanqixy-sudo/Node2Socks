use node2socks_core_adapter::{
    controller::MihomoController,
    mihomo::{MihomoConfig, MihomoManager},
    topology::{CoreSlot, CoreTopology, render_topology, slot_selector_name},
};
use std::{
    collections::HashSet, env, fs, net::TcpListener, path::PathBuf, process::Stdio, time::Duration,
};
use tokio::{process::Command, time::Instant};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 3 {
        return Err("usage: hundred-slot-smoke <mihomo.exe> <runtime-dir>".into());
    }
    let executable = PathBuf::from(&args[1]);
    let runtime = PathBuf::from(&args[2]);
    MihomoManager::new(MihomoConfig::new(&executable, &runtime))?.verify_binary()?;
    fs::create_dir_all(&runtime)?;
    let controller_port = reserve(&HashSet::new())?;
    let mut ports = Vec::new();
    let mut used = HashSet::from([controller_port]);
    while ports.len() < 100 {
        let port = reserve(&used)?;
        used.insert(port);
        ports.push(port)
    }
    let slots: Vec<_> = ports
        .iter()
        .map(|port| CoreSlot {
            id: Uuid::new_v4(),
            local_port: *port,
            selected: None,
        })
        .collect();
    let first = slots.first().unwrap().id;
    let last = slots.last().unwrap().id;
    let secret = Uuid::new_v4().simple().to_string();
    let config = render_topology(
        &CoreTopology {
            slots,
            available_nodes: vec![],
            download_port: None,
        },
        controller_port,
        &secret,
    )?;
    let path = runtime.join("hundred-slots.yaml");
    fs::write(&path, config)?;
    let mut command = Command::new(&executable);
    command
        .args(["-d"])
        .arg(&runtime)
        .args(["-f"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let started = Instant::now();
    let mut child = command.spawn()?;
    let pid = child.id().ok_or("missing pid")?;
    let controller = MihomoController::new(controller_port, secret)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if controller
            .selected(&slot_selector_name(first))
            .await
            .is_ok()
            && controller.selected(&slot_selector_name(last)).await.is_ok()
        {
            break;
        }
        if Instant::now() > deadline {
            child.start_kill()?;
            return Err("100 Slot startup timed out".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await
    }
    let startup_ms = started.elapsed().as_millis();
    if child.id() != Some(pid) {
        return Err("Core PID changed".into());
    }
    child.start_kill()?;
    child.wait().await?;
    for port in &ports {
        TcpListener::bind(("127.0.0.1", *port))?;
    }
    println!("result=PASS pid={pid} cores=1 slots=100 startup_ms={startup_ms} ports_released=100");
    Ok(())
}
fn reserve(excluded: &HashSet<u16>) -> std::io::Result<u16> {
    loop {
        let port = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
        if !excluded.contains(&port) {
            return Ok(port);
        }
    }
}
